//! In-memory PDF fixtures for unit tests.

use lopdf::{dictionary, Dictionary, Object, Stream};

/// Assembles a one-page PDF from a content stream, resources and extra page entries.
pub(crate) fn page_pdf(
    content: &[u8],
    resources: Dictionary,
    extra: Dictionary,
    mut doc: lopdf::Document,
) -> Vec<u8> {
    let pages_id = doc.new_object_id();
    let resources_id = doc.add_object(resources);
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
    let mut page = dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
    };
    page.extend(&extra);
    let page_id = doc.add_object(page);
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// An unembedded Helvetica font dictionary.
pub(crate) fn helvetica() -> Dictionary {
    dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" }
}

/// A 2x2 grey image XObject.
pub(crate) fn grey_image() -> Stream {
    Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
        },
        vec![0, 85, 170, 255],
    )
}

/// One page holding two text runs and one form XObject paint.
pub(crate) fn simple() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(helvetica());
    let form_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        },
        b"0 0 10 10 re f".to_vec(),
    ));
    let resources = dictionary! {
        "Font" => dictionary! { "F1" => font_id },
        "XObject" => dictionary! { "Fm1" => form_id },
    };
    let content =
        b"BT\n/F1 24 Tf\n72 700 Td\n(First run) Tj\n0 -30 Td\n(Second run) Tj\nET\n/Fm1 Do";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// One page with a crop box and a quarter-turn rotation.
pub(crate) fn rotated() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(helvetica());
    let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
    let extra = dictionary! {
        "Rotate" => 90,
        "CropBox" => vec![100.into(), 100.into(), 500.into(), 700.into()],
    };
    page_pdf(
        b"BT /F1 12 Tf 172 700 Td (Turned) Tj ET",
        resources,
        extra,
        doc,
    )
}

/// One page drawn only with path operators, as a design tool exports outlined text.
pub(crate) fn outlined() -> Vec<u8> {
    let mut content = Vec::new();
    for i in 0..300 {
        content.extend_from_slice(
            format!(
                "{} 100 m {} 110 l {} 120 100 130 90 120 c h f\n",
                i,
                i + 3,
                i + 5
            )
            .as_bytes(),
        );
    }
    page_pdf(
        &content,
        dictionary! {},
        dictionary! {},
        lopdf::Document::with_version("1.5"),
    )
}

/// One page whose text sits in a clip shorter than the font's ascent-to-descent box,
/// and a second run that the clip really does cut in half.
pub(crate) fn tight_clip() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(helvetica());
    let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
    let content = b"q 70 499 100 9 re W n BT /F1 12 Tf 72 500 Td (Tight) Tj ET Q\n\
        q 70 400 15 20 re W n BT /F1 12 Tf 72 405 Td (Halved) Tj ET Q\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// One page whose image and text each sit inside their own clip group, as Word exports them.
pub(crate) fn clipped() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(helvetica());
    let image_id = doc.add_object(grey_image());
    let resources = dictionary! {
        "Font" => dictionary! { "F1" => font_id },
        "XObject" => dictionary! { "Im1" => image_id },
    };
    let content = b"q 50 600 100 80 re W n q 100 0 0 80 50 600 cm /Im1 Do Q Q\n\
        q 60 480 200 40 re W n 0 0 1 rg BT /F1 12 Tf 72 500 Td (Clipped) Tj ( next) Tj ET Q\n\
        BT /F1 12 Tf 72 300 Td (Free) Tj ET\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// One page with a three-line paragraph, a block below it and a second column beside it.
pub(crate) fn paragraph() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(helvetica());
    let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
    let content = b"BT /F1 12 Tf 72 700 Td 14 TL\n\
        (The quick brown fox jumps over the lazy dog and) Tj T*\n\
        (keeps running through the quiet forest until it) Tj T*\n\
        (reaches the river.) Tj ET\n\
        BT /F1 12 Tf 72 640 Td (Next block stays.) Tj ET\n\
        BT /F1 12 Tf 400 700 Td (Right column) Tj 0 -14 Td (second line) Tj ET\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// One page with a line that changes font, size and colour from run to run, as a
/// reference list does: roman, bold, roman, bold in blue, smaller roman.
pub(crate) fn mixed_fonts() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let face =
        |name: &str| dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => name };
    let roman = doc.add_object(face("Times-Roman"));
    let bold = doc.add_object(face("Times-Bold"));
    let resources = dictionary! { "Font" => dictionary! { "F1" => roman, "F2" => bold } };
    let content = b"BT /F1 12 Tf 72 700 Td (6. ) Tj /F2 12 Tf (* ) Tj\n\
        /F1 12 Tf (Radha V et al., 2014, ) Tj\n\
        0 0 1 rg /F2 12 Tf (Indian J Hum Genet.) Tj\n\
        0 g /F1 10 Tf ( 20:37-42. ) Tj ET\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// One page with two lines whose second run does not start where the pen was left: a
/// table heading moved to its column with `Td`, and a run placed with `Td` after a
/// run that ends in a space widened by word spacing, set in a font of size one under
/// a scaling text matrix.
pub(crate) fn positioned_runs() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let face =
        |name: &str| dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => name };
    let regular = doc.add_object(face("Helvetica"));
    let bold = doc.add_object(face("Helvetica-Bold"));
    let resources = dictionary! { "Font" => dictionary! { "F1" => regular, "F2" => bold } };
    // "Agenda " is 44.7pt wide, so "File No." starts 15.3pt after it. "NICU: " ends
    // 3.049 units from the line start without its word spacing and 5.766 with it.
    let content = b"BT /F1 12 Tf 72 700 Td (Agenda ) Tj 60 0 Td (File No.) Tj ( ) Tj ET\n\
        BT 12 0 0 12 28 500 Tm /F2 1 Tf 2.717 Tw [(NIC)6 (U: )] TJ\n\
        /F1 1 Tf 0 Tw 3.1 0 Td [(Neonatal int)19 (ensive care unit)] TJ ET\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// An unembedded Courier font: every glyph is 600 units wide, 7.2pt at size 12.
fn courier() -> Dictionary {
    dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" }
}

/// One page with three two-line headings whose lines end in spaces: right-aligned with
/// the space in a run of its own, right-aligned with spaces at both ends of the run,
/// and centred. The visible text ends at x 500, ends at 485.6, and is centred on 300.
pub(crate) fn aligned_headings() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(courier());
    let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
    let content = b"BT /F1 12 Tf 356 700 Td (Vol. 15, No. ) Tj (3, 2024) Tj ( ) Tj ET\n\
        BT /F1 12 Tf 377.6 686 Td (Journal of Things) Tj ( ) Tj ET\n\
        BT /F1 12 Tf 334.4 600 Td (  SECOND HEADING LINE  ) Tj ET\n\
        BT /F1 12 Tf 384.8 586 Td (   HEADING ONE  ) Tj ET\n\
        BT /F1 12 Tf 238.8 500 Td (GUIDANCE DOCUMENT) Tj ( ) Tj ET\n\
        BT /F1 12 Tf 246 486 Td (FOR INSPECTIONS) Tj ( ) Tj ET\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// One page with two table header rows in Courier at size 10 (6pt a glyph). Cells of a
/// row are 15pt apart and read as one line; the last cell of each row is 21pt away and
/// is a line of its own. Three data rows start a column at x 117, 6pt right of the
/// second header cell.
pub(crate) fn table_rows() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(courier());
    let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
    let content = b"BT /F1 10 Tf 72 700 Td (Agen) Tj 39 0 Td (File Name) Tj ET\n\
        BT /F1 10 Tf 1 0 0 1 186 700 Tm (Firm) Tj ET\n\
        BT /F1 10 Tf 72 670 Td (1) Tj 45 0 Td (alpha) Tj 133 0 Td (one) Tj ET\n\
        BT /F1 10 Tf 72 640 Td (2) Tj 45 0 Td (beta) Tj 133 0 Td (two) Tj ET\n\
        BT /F1 10 Tf 72 610 Td (3) Tj 45 0 Td (gamma) Tj 133 0 Td (three) Tj ET\n\
        BT /F1 10 Tf 72 560 Td (Susceptible) Tj 81 0 Td (Intermediate) Tj 87 0 Td (Resistant) Tj ET\n\
        BT /F1 10 Tf 1 0 0 1 315 560 Tm (Susceptible) Tj ET\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// One page with a list item whose bullet is code 149 of a WinAnsi font that lists
/// widths from 32 on: code 127 also reads as a bullet and has no width.
pub(crate) fn bullet_item() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let widths: Vec<Object> = (32..=149)
        .map(|c| Object::Integer(if c == 127 { 0 } else { 500 }))
        .collect();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "Listing",
        "Encoding" => "WinAnsiEncoding", "FirstChar" => 32, "LastChar" => 149,
        "Widths" => widths,
    });
    let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
    let content = b"BT /F1 10 Tf 72 700 Td (\\225 Item) Tj ET\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// One page with text painted over other text: a small caption across a heading and
/// a word painted twice for weight. A superscript and a run that starts a fraction of
/// a point early stand for the overlaps that are ordinary typesetting.
pub(crate) fn overprinted() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(helvetica());
    let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
    // "Slight" is 25.01pt wide in Helvetica at 10pt; the next run starts 0.6pt inside it.
    let content = b"BT /F1 10 Tf 34 391 Td (What caused the most years lived with disability, by sex, in 2016?) Tj ET\n\
        BT /F1 6 Tf 238 392 Td (*COPD is chronic obstructive pulmonary disease.) Tj ET\n\
        BT /F1 10 Tf 34 300 Td (E = mc) Tj /F1 7 Tf 4 Ts (2) Tj 0 Ts /F1 10 Tf ( holds) Tj ET\n\
        BT /F1 10 Tf 34 250 Td (Slight) Tj ET BT /F1 10 Tf 58.41 250 Td (ly joined) Tj ET\n\
        BT /F1 10 Tf 34 200 Td (Heavy) Tj ET BT /F1 10 Tf 34.3 200 Td (Heavy) Tj ET\n";
    page_pdf(content, resources, dictionary! {}, doc)
}

/// A TrueType font with one empty glyph and the given embedding permissions (`fsType`).
pub(crate) fn tiny_font(fs_type: u16) -> Vec<u8> {
    let mut head = vec![0u8; 54];
    head[..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    let mut hhea = vec![0u8; 36];
    hhea[..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hhea[34..36].copy_from_slice(&1u16.to_be_bytes());
    let mut os2 = vec![0u8; 96];
    os2[..2].copy_from_slice(&3u16.to_be_bytes());
    os2[8..10].copy_from_slice(&fs_type.to_be_bytes());
    // The directory lists tables in tag order.
    let tables: [(&[u8; 4], Vec<u8>); 7] = [
        (b"OS/2", os2),
        (b"glyf", vec![0; 4]),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", vec![0x01, 0xF4, 0, 0]),
        (b"loca", vec![0; 4]),
        (b"maxp", vec![0, 0, 0x50, 0, 0, 1]),
    ];
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    let mut at = 12 + 16 * tables.len();
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(at as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        at += data.len().next_multiple_of(4);
    }
    for (_, data) in &tables {
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    out
}

/// One page with two images, an inline image, text in a nested form and kerned text.
pub(crate) fn rich() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let font_id = doc.add_object(helvetica());
    let image_id = doc.add_object(grey_image());
    let form_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 200.into(), 50.into()],
            "Matrix" => vec![1.into(), 0.into(), 0.into(), 1.into(), 300.into(), 400.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        },
        b"BT /F1 10 Tf 5 5 Td (In a form) Tj ET".to_vec(),
    ));
    let resources = dictionary! {
        "Font" => dictionary! { "F1" => font_id },
        "XObject" => dictionary! { "Im1" => image_id, "Fm1" => form_id },
    };
    let content = b"q 100 0 0 80 50 600 cm /Im1 Do Q\n\
        BT /F1 12 Tf 1 0 0 1 72 500 Tm (Alpha ) Tj (Beta) Tj 0 -20 Td [(Ga) -50 (mma)] TJ 14 TL (Delta) ' ET\n\
        q 40 0 0 40 300 600 cm /Im1 Do Q\n\
        /Fm1 Do\n\
        q 20 0 0 20 400 100 cm BI /W 2 /H 2 /BPC 8 /CS /G ID \x00\x55\xaa\xff EI Q\n";
    page_pdf(content, resources, dictionary! {}, doc)
}
