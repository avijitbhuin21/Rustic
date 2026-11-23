// Reads an `ask_user` tool call back out of the persisted transcript: the
// questions come from the call's input, the outcome from its result. The live
// question card exists only in memory, so after a restart this is all there is.

/** The questions array of an ask_user call, accepting a stringified array too. */
export function parseAskQuestions(input) {
  let questions = input?.questions;
  if (typeof questions === 'string') {
    try {
      questions = JSON.parse(questions);
    } catch {
      questions = null;
    }
  }
  if (!Array.isArray(questions)) return [];
  return questions.filter((q) => q && typeof q === 'object');
}

/** A string that is equal for two question lists with the same content. */
export function askQuestionsKey(questions) {
  try {
    return JSON.stringify(Array.isArray(questions) ? questions : []);
  } catch {
    return '';
  }
}

/** Coerces a string or { value, label, description } option into one shape. */
export function normalizeAskOption(opt) {
  if (opt && typeof opt === 'object') {
    const value = String(opt.value ?? opt.label ?? '');
    const label = String(opt.label ?? opt.value ?? '');
    const description = opt.description != null ? String(opt.description) : '';
    return { value, label, description };
  }
  const str = String(opt ?? '');
  return { value: str, label: str, description: '' };
}

/**
 * What became of an ask_user call. `state` is `answered` (with `answers`),
 * `dismissed`, `unanswered` (the run ended first) or `pending`.
 */
export function parseAskOutcome(output, { isError = false, interrupted = false } = {}) {
  if (output === undefined || output === null) {
    return { state: interrupted ? 'unanswered' : 'pending', answers: null };
  }
  const text = typeof output === 'string' ? output.trim() : '';
  if (text.startsWith('ASK_USER_CANCELLED')) return { state: 'dismissed', answers: null };
  if (!isError && text.startsWith('{')) {
    try {
      const answers = JSON.parse(text);
      if (answers && typeof answers === 'object' && !Array.isArray(answers)) {
        return { state: 'answered', answers };
      }
    } catch {
      /* not an answers object */
    }
  }
  return { state: 'unanswered', answers: null };
}

/** The answer to one question as a list of strings (empty when there is none). */
export function askAnswerValues(answers, question) {
  const a = answers?.[question?.id];
  if (Array.isArray(a)) return a.map((x) => String(x)).filter((x) => x.trim().length > 0);
  if (a != null && String(a).trim().length > 0) return [String(a)];
  return [];
}

const oneLine = (s) => String(s).replace(/\s*\n\s*/g, ' ').trim();
const quote = (s) => String(s).split('\n').map((l) => `> ${l}`).join('\n');

const KIND_HINT = { single: 'pick one', multi: 'pick any', free_text: 'free text' };

const OUTCOME_NOTE = {
  dismissed: '_Dismissed without an answer._',
  unanswered: '_The run stopped before this was answered._',
  pending: '_Waiting for an answer._',
};

/**
 * One ask_user call as markdown: each question, its options as a task list
 * with the chosen ones ticked, and typed answers quoted.
 */
export function askUserMarkdown(questions, outcome) {
  const list = Array.isArray(questions) ? questions : [];
  const answered = outcome?.state === 'answered';
  const parts = [];
  list.forEach((q, i) => {
    const hint = KIND_HINT[q.kind];
    const title = list.length > 1 ? `Question ${i + 1}` : 'Question';
    parts.push(`**${title}**${hint ? ` _(${hint})_` : ''}`);
    if (q.text) parts.push(String(q.text));

    const options = (Array.isArray(q.options) ? q.options : []).map(normalizeAskOption);
    const values = answered ? askAnswerValues(outcome.answers, q) : [];
    const chosen = new Set(values);
    const typed = values.filter((v) => !options.some((o) => o.value === v));

    if (options.length > 0) {
      const lines = options.map((o) => {
        const label = oneLine(o.label);
        const desc = o.description ? ` — ${oneLine(o.description)}` : '';
        return `- [${chosen.has(o.value) ? 'x' : ' '}] ${label}${desc}`;
      });
      for (const t of typed) lines.push(`- [x] Other: ${oneLine(t)}`);
      parts.push(lines.join('\n'));
    } else if (typed.length > 0) {
      parts.push(quote(typed.join('\n')));
    }
    if (answered && values.length === 0) parts.push('_No answer._');
  });
  const note = OUTCOME_NOTE[outcome?.state];
  if (note) parts.push(note);
  return parts.join('\n\n');
}

/** Answers written out as a chat message, for a question whose run is gone. */
export function askAnswersAsMessage(questions, answers) {
  const list = Array.isArray(questions) ? questions : [];
  const parts = ['Answers to your earlier questions:'];
  list.forEach((q, i) => {
    const values = askAnswerValues(answers, q);
    const answer = values.length === 0 ? '(no answer)' : values.join(q.kind === 'multi' ? '; ' : '\n');
    parts.push(`${i + 1}. ${oneLine(q.text || q.id || '')}\nAnswer: ${answer}`);
  });
  return parts.join('\n\n');
}
