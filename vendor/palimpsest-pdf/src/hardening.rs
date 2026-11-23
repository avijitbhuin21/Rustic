//! Hardening tests: encryption, signatures, tagging, resource limits and fuzzing.

use std::time::Instant;

use lopdf::{dictionary, EncryptionState, EncryptionVersion, Object, Permissions, Stream};

use crate::fixtures::{self, helvetica, page_pdf};
use crate::{Document, Error, Limits, Matrix, PageText, TextOptions};

/// Encrypts a PDF with RC4-128 and the given user password.
fn encrypt(bytes: &[u8], user: &str) -> Vec<u8> {
    let mut pdf = lopdf::Document::load_mem(bytes).unwrap();
    pdf.trailer.set(
        "ID",
        vec![
            Object::string_literal("0123456789abcdef"),
            Object::string_literal("0123456789abcdef"),
        ],
    );
    let version = EncryptionVersion::V2 {
        document: &pdf,
        owner_password: "owner",
        user_password: user,
        key_length: 128,
        permissions: Permissions::default(),
    };
    let state = EncryptionState::try_from(version).unwrap();
    pdf.encrypt(&state).unwrap();
    let mut out = Vec::new();
    pdf.save_to(&mut out).unwrap();
    out
}

fn texts(doc: &mut Document) -> Vec<String> {
    doc.page_objects(1)
        .unwrap()
        .objects
        .iter()
        .filter_map(|o| o.as_text().map(|t| t.text.clone()))
        .collect()
}

#[test]
fn opens_encrypted_documents_and_saves_them_without_encryption() {
    let locked = encrypt(&fixtures::simple(), "secret");
    assert!(locked.windows(8).any(|w| w == b"/Encrypt"));
    assert!(matches!(
        Document::from_bytes(&locked),
        Err(Error::PasswordRequired)
    ));
    assert!(matches!(
        Document::open(&locked, Some("wrong")),
        Err(Error::WrongPassword)
    ));
    let mut doc = Document::open(&locked, Some("secret")).unwrap();
    assert!(doc.info().was_encrypted);
    assert_eq!(texts(&mut doc), ["First run", "Second run"]);
    let id = doc.page_objects(1).unwrap().objects[0].id;
    doc.replace_line(1, id, "First ruin", &TextOptions::default())
        .unwrap();
    let saved = doc.save().unwrap();
    assert!(!saved.windows(8).any(|w| w == b"/Encrypt"));
    let mut plain = Document::from_bytes(&saved).unwrap();
    assert!(!plain.info().was_encrypted);
    assert_eq!(texts(&mut plain), ["First ruin", "Second run"]);
    // An empty user password needs no password from the host.
    let mut open = Document::from_bytes(&encrypt(&fixtures::simple(), "")).unwrap();
    assert!(open.info().was_encrypted);
    assert_eq!(texts(&mut open), ["First run", "Second run"]);
}

#[test]
fn reads_and_rewrites_object_stream_files() {
    let mut pdf = lopdf::Document::load_mem(&fixtures::rich()).unwrap();
    let mut modern = Vec::new();
    pdf.save_modern(&mut modern).unwrap();
    assert!(modern.windows(7).any(|w| w == b"/ObjStm"));
    let mut doc = Document::from_bytes(&modern).unwrap();
    let before = texts(&mut doc);
    assert_eq!(before, ["Alpha ", "Beta", "Gamma", "Delta", "In a form"]);
    let id = doc
        .page_objects(1)
        .unwrap()
        .objects
        .iter()
        .find(|o| o.as_image().is_some())
        .unwrap()
        .id;
    doc.translate(1, id, 10.0, 10.0).unwrap();
    let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
    assert_eq!(texts(&mut again), before);
}

#[test]
fn reports_signatures_and_tagging_and_keeps_marked_content() {
    let plain = Document::from_bytes(&fixtures::simple()).unwrap().info();
    assert!(!plain.signed && !plain.tagged && !plain.was_encrypted && plain.pages == 1);

    let mut pdf = lopdf::Document::with_version("1.7");
    let font_id = pdf.add_object(helvetica());
    let sig = pdf.add_object(dictionary! {
        "Type" => "Sig",
        "Filter" => "Adobe.PPKLite",
        "ByteRange" => vec![0.into(), 10.into(), 20.into(), 30.into()],
        "Contents" => Object::string_literal("00"),
    });
    let field = pdf.add_object(
        dictionary! { "FT" => "Sig", "T" => Object::string_literal("Signature1"), "V" => sig },
    );
    let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
    let content = b"/P <</MCID 0>> BDC BT /F1 12 Tf 72 700 Td (Tagged paragraph) Tj ET EMC\n/P <</MCID 1>> BDC BT /F1 12 Tf 72 680 Td (Second one) Tj ET EMC";
    let bytes = page_pdf(content, resources, dictionary! {}, pdf);
    let mut pdf = lopdf::Document::load_mem(&bytes).unwrap();
    let tree = pdf.add_object(dictionary! { "Type" => "StructTreeRoot" });
    let root = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let catalog = pdf.get_dictionary_mut(root).unwrap();
    catalog.set(
        "AcroForm",
        dictionary! { "Fields" => vec![field.into()], "SigFlags" => 3 },
    );
    catalog.set("StructTreeRoot", tree);
    catalog.set("MarkInfo", dictionary! { "Marked" => true });
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();

    let mut doc = Document::from_bytes(&bytes).unwrap();
    let info = doc.info();
    assert!(info.signed && info.tagged);
    // Edits replace show operators only, so marked-content sequences stay as they were
    // and the structure tree keeps pointing at content that exists.
    let ids: Vec<u32> = doc
        .page_objects(1)
        .unwrap()
        .objects
        .iter()
        .map(|o| o.id)
        .collect();
    doc.replace_line(
        1,
        ids[0],
        "Tagged paragraph, edited",
        &TextOptions::default(),
    )
    .unwrap();
    doc.translate(1, ids[1], 5.0, -5.0).unwrap();
    doc.delete(1, ids[1]).unwrap();
    let page = doc.page_id(1).unwrap();
    let content = String::from_utf8_lossy(&doc.page_content(page)).into_owned();
    assert_eq!(
        (
            content.matches("BDC").count(),
            content.matches("EMC").count()
        ),
        (2, 2)
    );
    assert!(content.contains("/MCID 0") && content.contains("/MCID 1"));
    let saved = Document::from_bytes(&doc.save().unwrap()).unwrap().info();
    assert!(saved.tagged && saved.signed);
}

#[test]
fn stays_within_its_budgets() {
    // Ten forms that each paint the next one ten times: 10^10 operations if followed.
    let mut pdf = lopdf::Document::with_version("1.5");
    let font_id = pdf.add_object(helvetica());
    let mut next = pdf.add_object(Stream::new(
        dictionary! { "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 600.into(), 800.into()], "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } } },
        b"BT /F1 8 Tf 10 10 Td (x) Tj ET".to_vec(),
    ));
    for _ in 0..10 {
        let dict = dictionary! { "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 600.into(), 800.into()], "Resources" => dictionary! { "XObject" => dictionary! { "F" => next } } };
        next = pdf.add_object(Stream::new(dict, b"/F Do ".repeat(10)));
    }
    // A form that paints itself, and a megabyte of zeros that inflates from a kilobyte.
    let own = pdf.new_object_id();
    let dict = dictionary! { "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 9.into(), 9.into()], "Resources" => dictionary! { "XObject" => dictionary! { "S" => own } } };
    pdf.objects.insert(
        own,
        Object::Stream(Stream::new(dict, b"/S Do /S Do".to_vec())),
    );
    let mut bomb = Stream::new(
        dictionary! { "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 9.into(), 9.into()] },
        vec![b' '; 4 << 20],
    );
    bomb.compress().unwrap();
    let bomb = pdf.add_object(bomb);
    let resources =
        dictionary! { "XObject" => dictionary! { "Deep" => next, "Self" => own, "Bomb" => bomb } };
    let bytes = page_pdf(
        b"/Self Do /Bomb Do /Deep Do",
        resources,
        dictionary! {},
        pdf,
    );

    let started = Instant::now();
    let mut doc = Document::from_bytes(&bytes).unwrap();
    doc.set_limits(Limits {
        max_ops: 50_000,
        max_glyphs: 1_000,
        max_stream_bytes: 1 << 20,
        max_depth: 12,
    });
    let page = doc.page_objects(1).unwrap();
    assert!(page.truncated && !page.warnings.is_empty());
    assert!(page.objects.len() <= 1_000);
    assert!(
        started.elapsed().as_secs() < 5,
        "took {:?}",
        started.elapsed()
    );
    // With the default budget the page still finishes, cut off at the operation limit.
    let mut doc = Document::from_bytes(&bytes).unwrap();
    doc.set_limits(Limits {
        max_ops: 400_000,
        ..Limits::default()
    });
    assert!(doc.page_objects(1).unwrap().truncated);
}

/// A small deterministic generator, so failures can be reproduced from the seed.
pub(crate) struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Opens `bytes` and, when that works, reads and edits every page. Must not panic.
pub(crate) fn exercise(bytes: &[u8], rng: &mut Rng) {
    let Ok(mut doc) = Document::from_bytes(bytes) else {
        return;
    };
    doc.set_limits(Limits {
        max_ops: 200_000,
        max_glyphs: 50_000,
        max_stream_bytes: 8 << 20,
        max_depth: 6,
    });
    let _ = doc.info();
    let _ = doc.inspect();
    for page in 1..=doc.page_count().min(3) as u32 {
        let Ok(objects) = doc.page_objects(page) else {
            continue;
        };
        let _ = objects.to_json();
        let ids: Vec<u32> = objects.objects.iter().map(|o| o.id).collect();
        let _ = doc.page_layout(page).map(|l| l.to_json());
        for _ in 0..4.min(ids.len()) {
            let id = ids[rng.below(ids.len())];
            let _ = match rng.below(6) {
                0 => doc.delete(page, id),
                1 => doc.translate(page, id, 31.5, -17.25).map(|_| ()),
                2 => doc
                    .transform(page, id, &Matrix([0.5, 0.2, -0.3, 1.5, 4.0, 9.0]))
                    .map(|_| ()),
                3 => doc
                    .replace_line(
                        page,
                        id,
                        "Edited by the fuzzer \u{20AC}",
                        &TextOptions::default(),
                    )
                    .map(|_| ()),
                4 => doc
                    .replace_block(
                        page,
                        id,
                        "One two three four five six seven eight nine ten eleven twelve.\nNext.",
                        &TextOptions::default(),
                    )
                    .map(|_| ()),
                _ => doc.undo().map(|_| ()).ok_or(Error::NoSuchPage(0)),
            };
        }
    }
    if let Ok(saved) = doc.save() {
        // What the engine writes, it must be able to read back.
        assert!(
            Document::from_bytes(&saved).is_ok(),
            "saved output does not reopen"
        );
    }
    while doc.undo().is_some() {}
}

#[test]
fn survives_mutated_files_and_random_content() {
    let started = Instant::now();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let seeds = [
        fixtures::simple(),
        fixtures::rich(),
        fixtures::clipped(),
        fixtures::paragraph(),
        fixtures::rotated(),
        encrypt(&fixtures::rich(), ""),
    ];
    assert!(
        seeds.iter().all(|s| s.len() > 400),
        "seed sizes {:?}",
        seeds.iter().map(Vec::len).collect::<Vec<_>>()
    );
    let mut opened = 0;
    for round in 0..1500 {
        let mut bytes = seeds[round % seeds.len()].clone();
        // Two rounds in three only overwrite bytes, which keeps cross-reference offsets
        // valid, so the damage reaches the interpreter instead of stopping at the parser.
        let structural = round % 3 == 0;
        for _ in 0..1 + rng.below(6) {
            if bytes.is_empty() {
                break;
            }
            let at = rng.below(bytes.len());
            match if structural { rng.below(5) } else { 0 } {
                0 => bytes[at] = rng.next() as u8,
                1 => bytes.truncate(at.max(8)),
                2 => {
                    let alphabet = b"<[(/ \n0-9.eEQq";
                    bytes.insert(at, alphabet[rng.below(alphabet.len())]);
                }
                3 => {
                    let end = (at + rng.below(40)).min(bytes.len());
                    bytes.drain(at..end);
                }
                _ => {
                    let from = rng.below(bytes.len());
                    let chunk: Vec<u8> = bytes[from..(from + 24).min(bytes.len())].to_vec();
                    bytes.splice(at..at, chunk);
                }
            }
        }
        opened += usize::from(Document::from_bytes(&bytes).is_ok());
        exercise(&bytes, &mut rng);
    }
    // Random operator soup as page content, with valid surroundings.
    let words: [&[u8]; 40] = [
        b"q",
        b"Q",
        b"BT",
        b"ET",
        b"/F1 12 Tf",
        b"/F9 -3 Tf",
        b"72 700 Td",
        b"0 -14 TD",
        b"T*",
        b"(text) Tj",
        b"[(a) -50 (b) 9e9 <4142>] TJ",
        b"(q) '",
        b"1 2 (r) \"",
        b"1 0 0 1 5 5 cm",
        b"0 0 0 0 0 0 cm",
        b"2 0 0 2 0 0 Tm",
        b"1e30 0 0 1e-30 0 0 Tm",
        b"/Im1 Do",
        b"/Fm1 Do",
        b"/Nope Do",
        b"0 0 50 50 re",
        b"W n",
        b"W* n",
        b"f",
        b"S",
        b"3 Tr",
        b"50 Tz",
        b"0 Tz",
        b"-5 Ts",
        b"4 Tc",
        b"9 Tw",
        b"0.5 g",
        b"1 0 0 rg",
        b"/Pattern cs",
        b"/P1 scn",
        b"BI /W 2 /H 2 /BPC 8 /CS /G ID \x00\x01\x02\x03 EI",
        b"BI /W 9",
        b"(unterminated",
        b"<<",
        b"]",
    ];
    for _ in 0..1500 {
        let mut content = Vec::new();
        for _ in 0..rng.below(60) {
            content.extend_from_slice(words[rng.below(words.len())]);
            content.push(if rng.below(9) == 0 { b'\n' } else { b' ' });
        }
        let mut pdf = lopdf::Document::with_version("1.5");
        let font = pdf.add_object(helvetica());
        let image = pdf.add_object(fixtures::grey_image());
        let form = pdf.add_object(Stream::new(dictionary! { "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 90.into(), 90.into()] }, b"BT /F1 9 Tf (in form) Tj ET".to_vec()));
        let resources = dictionary! { "Font" => dictionary! { "F1" => font }, "XObject" => dictionary! { "Im1" => image, "Fm1" => form } };
        let bytes = page_pdf(
            &content,
            resources,
            dictionary! { "Rotate" => (rng.below(5) * 90) as i64 },
            pdf,
        );
        exercise(&bytes, &mut rng);
        let mut doc = Document::from_bytes(&bytes).unwrap();
        let page = doc.page_objects(1).unwrap();
        assert!(matches!(
            page.text,
            PageText::Text
                | PageText::HiddenText
                | PageText::Empty
                | PageText::Scanned
                | PageText::Outlined
        ));
        assert!(page.objects.iter().all(|o| o.id > 0));
    }
    println!(
        "mutated files that still opened: {opened} of 1500; random content pages: 1500; time {:?}",
        started.elapsed()
    );
    assert!(opened > 100, "only {opened} mutated files still opened; the mutations are too destructive to test anything");
    assert!(
        started.elapsed().as_secs() < 120,
        "fuzzing took {:?}",
        started.elapsed()
    );
}

/// A longer fuzzing run over real documents. Run with
/// `cargo test --release -p palimpsest-pdf -- --ignored fuzzes_the_private_corpus --nocapture`.
#[test]
#[ignore = "needs documents in corpus/private and takes minutes"]
fn fuzzes_the_private_corpus() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/private");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let (mut files, mut rounds, mut opened) = (0, 0, 0);
    let started = Instant::now();
    for entry in entries.flatten() {
        let Ok(original) = std::fs::read(entry.path()) else {
            continue;
        };
        if original.len() > 2 << 20 || entry.path().extension().is_none_or(|e| e != "pdf") {
            continue;
        }
        files += 1;
        exercise(&original, &mut rng);
        for _ in 0..600 {
            let mut bytes = original.clone();
            for _ in 0..1 + rng.below(12) {
                let at = rng.below(bytes.len());
                match rng.below(3) {
                    0 => bytes[at] = rng.next() as u8,
                    1 => bytes[at] ^= 1 << rng.below(8),
                    _ => {
                        let end = (at + rng.below(200)).min(bytes.len());
                        bytes.drain(at..end);
                    }
                }
            }
            rounds += 1;
            opened += usize::from(Document::from_bytes(&bytes).is_ok());
            exercise(&bytes, &mut rng);
        }
    }
    println!(
        "fuzzed {files} files, {rounds} mutations, {opened} still opened, in {:?}",
        started.elapsed()
    );
}
