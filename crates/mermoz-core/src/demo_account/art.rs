//! Files the demo mail carries, drawn here so nothing is fetched or bundled: PNG
//! pictures (banners, logos, photos of made-up things), one-page PDFs and calendar
//! invitations. Everything is a pure function of its inputs, so the demo is the same
//! on every machine.

/// An RGB color.
pub type Rgb = [u8; 3];

/// A picture drawn in memory, then encoded as PNG.
pub struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    /// A canvas filled with a top-to-bottom gradient.
    pub fn gradient(w: usize, h: usize, top: Rgb, bottom: Rgb) -> Self {
        let mut px = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            let t = y as f32 / (h.max(2) - 1) as f32;
            let c: Vec<u8> = (0..3)
                .map(|i| (top[i] as f32 + (bottom[i] as f32 - top[i] as f32) * t).round() as u8)
                .collect();
            for _ in 0..w {
                px.extend_from_slice(&c);
            }
        }
        Canvas { w, h, px }
    }

    fn set(&mut self, x: i64, y: i64, c: Rgb) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 3;
        self.px[i..i + 3].copy_from_slice(&c);
    }

    /// A filled rectangle with rounded corners of radius `r`.
    pub fn rect(&mut self, x: i64, y: i64, w: i64, h: i64, r: i64, c: Rgb) -> &mut Self {
        for yy in y..y + h {
            for xx in x..x + w {
                let dx = (x + r - xx).max(xx - (x + w - 1 - r)).max(0);
                let dy = (y + r - yy).max(yy - (y + h - 1 - r)).max(0);
                if dx * dx + dy * dy <= r * r {
                    self.set(xx, yy, c);
                }
            }
        }
        self
    }

    /// A filled disc.
    pub fn disc(&mut self, cx: i64, cy: i64, r: i64, c: Rgb) -> &mut Self {
        for y in cy - r..=cy + r {
            for x in cx - r..=cx + r {
                if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r {
                    self.set(x, y, c);
                }
            }
        }
        self
    }

    pub fn png(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity((self.w * 3 + 1) * self.h);
        for row in self.px.chunks(self.w * 3) {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&(self.w as u32).to_be_bytes());
        ihdr.extend_from_slice(&(self.h as u32).to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &zlib(&raw, self.w * 3 + 1));
        chunk(&mut out, b"IEND", &[]);
        out
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_in = kind.to_vec();
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Bits written least significant first, as deflate wants.
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, n: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// A Huffman code, which deflate stores most significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        let mut rev = 0;
        for i in 0..len {
            rev |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.put(rev, len);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn literal(b: &mut Bits, v: u32) {
    match v {
        0..=143 => b.code(0x30 + v, 8),
        144..=255 => b.code(0x190 + v - 144, 9),
        256..=279 => b.code(v - 256, 7),
        _ => b.code(0xC0 + v - 280, 8),
    }
}

/// zlib stream with one fixed-Huffman deflate block. Matches are only looked for one
/// pixel back and one row up, which is where drawn pictures repeat themselves.
fn zlib(data: &[u8], row: usize) -> Vec<u8> {
    let mut b = Bits {
        out: vec![0x78, 0x01],
        acc: 0,
        n: 0,
    };
    b.put(1, 1);
    b.put(1, 2);
    let mut i = 0;
    while i < data.len() {
        let mut best = (0usize, 0usize);
        for d in [3usize, row] {
            if d == 0 || d > i || d > 32_768 {
                continue;
            }
            let mut l = 0;
            while l < 258 && i + l < data.len() && data[i + l] == data[i + l - d] {
                l += 1;
            }
            if l > best.0 {
                best = (l, d);
            }
        }
        let (len, dist) = best;
        if len >= 3 {
            let li = LEN_BASE.iter().rposition(|&x| x as usize <= len).unwrap();
            literal(&mut b, 257 + li as u32);
            b.put((len - LEN_BASE[li] as usize) as u32, LEN_EXTRA[li] as u32);
            let di = DIST_BASE.iter().rposition(|&x| x as usize <= dist).unwrap();
            b.code(di as u32, 5);
            b.put(
                (dist - DIST_BASE[di] as usize) as u32,
                DIST_EXTRA[di] as u32,
            );
            i += len;
        } else {
            literal(&mut b, data[i] as u32);
            i += 1;
        }
    }
    literal(&mut b, 256);
    let mut out = b.finish();
    let (mut a, mut s) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65_521;
        s = (s + a) % 65_521;
    }
    out.extend_from_slice(&((s << 16) | a).to_be_bytes());
    out
}

/// A wide header picture: the brand's colors, a few soft shapes.
pub fn banner(top: Rgb, bottom: Rgb, accent: Rgb, seed: u64) -> Vec<u8> {
    let (w, h) = (600, 160);
    let mut c = Canvas::gradient(w, h, top, bottom);
    let mut r = seed;
    for i in 0..5 {
        r = r.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let x = 60 + (r >> 33) as i64 % 520;
        let y = 30 + (r >> 17) as i64 % 100;
        let rad = 14 + (r >> 9) as i64 % 34;
        let shade = if i % 2 == 0 {
            accent
        } else {
            mix(accent, bottom)
        };
        c.disc(x, y, rad, shade);
    }
    c.rect(40, 118, 220, 14, 7, [255, 255, 255]);
    c.rect(40, 138, 140, 8, 4, mix([255, 255, 255], bottom));
    c.png()
}

/// A made-up photo: a tiled surface (a kitchen wall, a floor sample) in two colors.
pub fn tiles(a: Rgb, b: Rgb, grout: Rgb) -> Vec<u8> {
    let (w, h) = (480, 320);
    let mut c = Canvas::gradient(w, h, grout, grout);
    for row in 0..8 {
        for col in 0..12 {
            let color = if (row + col) % 3 == 0 { b } else { a };
            c.rect(col * 40 + 2, row * 40 + 2, 36, 36, 3, color);
        }
    }
    c.png()
}

/// A made-up photo: a landscape of hills under a sky, with a sun.
pub fn landscape(sky: Rgb, hills: Rgb, sun: Rgb) -> Vec<u8> {
    let (w, h) = (480, 320);
    let mut c = Canvas::gradient(w, h, sky, mix(sky, [255, 255, 255]));
    c.disc(360, 90, 38, sun);
    c.disc(120, 420, 230, mix(hills, [255, 255, 255]));
    c.disc(380, 460, 260, hills);
    c.png()
}

fn mix(a: Rgb, b: Rgb) -> Rgb {
    [
        ((a[0] as u16 + b[0] as u16) / 2) as u8,
        ((a[1] as u16 + b[1] as u16) / 2) as u8,
        ((a[2] as u16 + b[2] as u16) / 2) as u8,
    ]
}

/// A one-page PDF: a title and lines of text in Helvetica.
pub fn pdf(title: &str, lines: &[String]) -> Vec<u8> {
    let esc = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii())
            .collect::<String>()
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)")
    };
    let mut content = format!("BT /F1 20 Tf 56 770 Td ({}) Tj ET\n", esc(title));
    for (i, l) in lines.iter().enumerate() {
        content.push_str(&format!(
            "BT /F1 11 Tf 56 {} Td ({}) Tj ET\n",
            730 - 18 * i as i64,
            esc(l)
        ));
    }
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![];
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A calendar invitation (`text/calendar`) for one event.
pub fn ics(
    uid: &str,
    summary: &str,
    start: chrono::DateTime<chrono::Utc>,
    minutes: i64,
    organizer: &str,
    place: &str,
) -> Vec<u8> {
    let fmt = |d: chrono::DateTime<chrono::Utc>| d.format("%Y%m%dT%H%M%SZ").to_string();
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Mermoz demo//EN\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:{}\r\nDTSTART:{}\r\nDTEND:{}\r\nSUMMARY:{summary}\r\nLOCATION:{place}\r\nORGANIZER:mailto:{organizer}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        fmt(start),
        fmt(start),
        fmt(start + chrono::Duration::minutes(minutes)),
    )
    .into_bytes()
}
