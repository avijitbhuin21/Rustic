//! Pool of tree-sitter `Parser`s keyed by language name.
//!
//! `Parser::new()` followed by `set_language(...)` is cheap individually but
//! adds up when every parse pays for it. Concurrent tasks in the same
//! project share one pool and rent a parser per parse; idle parsers stay
//! parked between requests.
//!
//! M1: storage is a `DashMap<String, Vec<Parser>>`. Per-language buckets
//! lock independently, so two concurrent parses in different languages
//! never contend; two parses in the same language only contend on the
//! single bucket's RwLock for the duration of a `Vec::pop` / `Vec::push`.
//! Previously a single `Mutex<HashMap>` serialised every parse across
//! every task in every language.

use dashmap::DashMap;
use tree_sitter::{Language, Parser};

/// Pool of parked `Parser`s, grouped by language name. Buckets are per-
/// language `Vec<Parser>` (LIFO — newest first), each guarded by its own
/// DashMap shard lock. Idle parsers stay parked between requests.
#[derive(Default)]
pub struct ParserPool {
    inner: DashMap<String, Vec<Parser>>,
}

impl ParserPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `f` against a parser configured for `language`. Reuses a parked
    /// parser when one is available; otherwise constructs a fresh one. The
    /// parser is returned to the pool when `f` completes (or dropped on
    /// panic — we don't go out of our way to recover, since tree-sitter
    /// parses don't panic in practice).
    ///
    /// Returns `None` if `set_language` fails (ABI mismatch between the
    /// `tree-sitter` runtime and the grammar — bumping one without the
    /// other is the usual cause).
    pub fn with_parser<F, T>(&self, lang_name: &str, language: Language, f: F) -> Option<T>
    where
        F: FnOnce(&mut Parser) -> Option<T>,
    {
        // Phase 1: pop a parked parser from the language's bucket (if any).
        // The shard-level lock is held only for the pop.
        let mut parser = self
            .inner
            .get_mut(lang_name)
            .and_then(|mut bucket| bucket.pop())
            .unwrap_or_else(Parser::new);

        // Always re-set the language — cheap, and the pool may have been
        // shared with another grammar revision after a hot reload.
        if parser.set_language(&language).is_err() {
            tracing::warn!(
                lang = %lang_name,
                "tree-sitter set_language failed; dropping parser"
            );
            return None;
        }

        let result = f(&mut parser);

        // Phase 3: return the parser. The dashmap entry API gives us
        // get-or-insert with a single shard lock acquisition.
        self.inner
            .entry(lang_name.to_string())
            .or_default()
            .push(parser);
        result
    }

    /// Number of parked parsers across all languages. Diagnostic only.
    pub fn parked(&self) -> usize {
        self.inner.iter().map(|kv| kv.value().len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_returns_to_pool_after_use() {
        let pool = ParserPool::new();
        let lang = rustic_core::syntax::LanguageRegistry::get_language("rust")
            .expect("rust grammar available");
        assert_eq!(pool.parked(), 0);
        let _ = pool.with_parser("rust", lang, |p| p.parse(b"fn main() {}", None));
        assert_eq!(pool.parked(), 1);
        // Second call reuses the parked parser.
        let _ = pool.with_parser("rust", rustic_core::syntax::LanguageRegistry::get_language("rust").unwrap(), |p| p.parse(b"fn x() {}", None));
        assert_eq!(pool.parked(), 1);
    }

    #[test]
    fn separate_languages_get_separate_buckets() {
        let pool = ParserPool::new();
        let rust = rustic_core::syntax::LanguageRegistry::get_language("rust").unwrap();
        let py = rustic_core::syntax::LanguageRegistry::get_language("python").unwrap();
        let _ = pool.with_parser("rust", rust, |p| p.parse(b"fn main() {}", None));
        let _ = pool.with_parser("python", py, |p| p.parse(b"def f(): pass", None));
        assert_eq!(pool.parked(), 2);
    }

    // M1 contention smoke-test: 8 threads parsing the same language
    // shouldn't deadlock and should leave the pool in a consistent state.
    #[test]
    fn many_threads_same_language_no_deadlock() {
        use std::sync::Arc;
        use std::thread;
        let pool = Arc::new(ParserPool::new());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let p = Arc::clone(&pool);
            handles.push(thread::spawn(move || {
                let lang = rustic_core::syntax::LanguageRegistry::get_language("rust").unwrap();
                for _ in 0..50 {
                    let _ = p.with_parser("rust", lang.clone(), |parser| {
                        parser.parse(b"fn main() {}", None)
                    });
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // The pool should hold AT LEAST one parser (likely up to 8 if all
        // threads were in flight simultaneously). Just sanity-check it
        // isn't empty and isn't pathological.
        let parked = pool.parked();
        assert!(parked >= 1, "no parsers parked after 8x50 parses");
        assert!(parked <= 8, "more parked than peak concurrency: {}", parked);
    }
}
