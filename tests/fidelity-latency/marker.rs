//! Background-colour cell marker painted by the PTY helper and read back from a compositor
//! capture. 48 cells on row 4 (0-based), starting at column 3 (0-based): magic 0xA5, seq u16,
//! !seq u16, suffix 0x5A, MSB first, one bit per cell (SPACE with a truecolor background).

use serde_json::Value;

pub const MAGIC: u8 = 0xA5;
pub const SUFFIX: u8 = 0x5A;
pub const CELLS: usize = 48;
pub const ROW: u32 = 4;
pub const COL: u32 = 3;
pub const MIN_COLS: u32 = COL + CELLS as u32 + 1;
pub const MIN_ROWS: u32 = ROW + 1;
pub const ON_RGB: [u8; 3] = [0xF8, 0xFA, 0xFC];
pub const OFF_RGB: [u8; 3] = [0x1E, 0x29, 0x3B];
/// Per-channel tolerance; widening it after a failure requires root review.
pub const TOLERANCE: u8 = 3;

pub fn bits(seq: u16) -> [bool; CELLS] {
    let bytes = [
        MAGIC,
        (seq >> 8) as u8,
        seq as u8,
        !(seq >> 8) as u8,
        !seq as u8,
        SUFFIX,
    ];
    let mut out = [false; CELLS];
    for (i, bit) in out.iter_mut().enumerate() {
        *bit = bytes[i / 8] & (0x80 >> (i % 8)) != 0;
    }
    out
}

/// Exact bytes the helper writes for `seq` (cursor hiding is written once before seq 0).
pub fn ansi(seq: u16) -> Vec<u8> {
    let mut s = format!("\x1b[{};{}H", ROW + 1, COL + 1);
    for bit in bits(seq) {
        let [r, g, b] = if bit { ON_RGB } else { OFF_RGB };
        s.push_str(&format!("\x1b[48;2;{r};{g};{b}m "));
    }
    s.push_str("\x1b[0m");
    s.into_bytes()
}

/// What the page reports (CSS px, relative to the web view viewport).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageObservation {
    pub dpr: f64,
    pub canvas_left_css: f64,
    pub canvas_top_css: f64,
    pub cell_w_css: f64,
    pub cell_h_css: f64,
    pub cols: u32,
    pub rows: u32,
}

impl PageObservation {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let f = |k: &str| {
            v.get(k)
                .and_then(Value::as_f64)
                .ok_or(format!("page: missing {k}"))
        };
        let u = |k: &str| {
            v.get(k)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(format!("page: missing {k}"))
        };
        Ok(Self {
            dpr: f("dpr")?,
            canvas_left_css: f("canvas_left_css")?,
            canvas_top_css: f("canvas_top_css")?,
            cell_w_css: f("cell_w_css")?,
            cell_h_css: f("cell_h_css")?,
            cols: u("cols")?,
            rows: u("rows")?,
        })
    }
}

/// Terminal cell grid in capture pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellGrid {
    pub origin_x: f64,
    pub origin_y: f64,
    pub cell_w: f64,
    pub cell_h: f64,
}

/// `viewport_offset_px`: web view viewport origin inside the captured output, measured by the
/// parent (window position + GTK CSD). `None` is refused: zero is never assumed.
pub fn grid(
    page: &PageObservation,
    viewport_offset_px: Option<(f64, f64)>,
) -> Result<CellGrid, String> {
    let (ox, oy) = viewport_offset_px.ok_or("viewport offset not measured")?;
    let finite_pos = |x: f64| x.is_finite() && x > 0.0;
    if !finite_pos(page.dpr) || !finite_pos(page.cell_w_css) || !finite_pos(page.cell_h_css) {
        return Err(format!("page metrics invalid: {page:?}"));
    }
    if page.cols < MIN_COLS || page.rows < MIN_ROWS {
        return Err(format!(
            "geometry {}x{} below {MIN_COLS}x{MIN_ROWS}",
            page.cols, page.rows
        ));
    }
    if !(ox.is_finite()
        && oy.is_finite()
        && page.canvas_left_css.is_finite()
        && page.canvas_top_css.is_finite())
    {
        return Err("offset not finite".into());
    }
    Ok(CellGrid {
        origin_x: ox + page.canvas_left_css * page.dpr,
        origin_y: oy + page.canvas_top_css * page.dpr,
        cell_w: page.cell_w_css * page.dpr,
        cell_h: page.cell_h_css * page.dpr,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct RgbView<'a> {
    pub width: usize,
    pub height: usize,
    pub rgb: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerError {
    OutOfBounds,
    Ambiguous { cell: usize, rgb: [u8; 3] },
    Prefix(u8),
    Complement { seq: u16, inverse: u16 },
    Suffix(u8),
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= TOLERANCE)
}

pub fn decode(img: RgbView, grid: &CellGrid) -> Result<u16, MarkerError> {
    if img.rgb.len() != img.width * img.height * 3 {
        return Err(MarkerError::OutOfBounds);
    }
    let y = (grid.origin_y + (ROW as f64 + 0.5) * grid.cell_h).floor();
    let mut bytes = [0u8; 6];
    for cell in 0..CELLS {
        let x = (grid.origin_x + (COL as f64 + cell as f64 + 0.5) * grid.cell_w).floor();
        if !(x >= 0.0 && y >= 0.0 && (x as usize) < img.width && (y as usize) < img.height) {
            return Err(MarkerError::OutOfBounds);
        }
        let i = (y as usize * img.width + x as usize) * 3;
        let rgb = [img.rgb[i], img.rgb[i + 1], img.rgb[i + 2]];
        let bit = match (near(rgb, ON_RGB), near(rgb, OFF_RGB)) {
            (true, false) => true,
            (false, true) => false,
            _ => return Err(MarkerError::Ambiguous { cell, rgb }),
        };
        bytes[cell / 8] = (bytes[cell / 8] << 1) | bit as u8;
    }
    if bytes[0] != MAGIC {
        return Err(MarkerError::Prefix(bytes[0]));
    }
    let seq = u16::from_be_bytes([bytes[1], bytes[2]]);
    let inverse = u16::from_be_bytes([bytes[3], bytes[4]]);
    if inverse != !seq {
        return Err(MarkerError::Complement { seq, inverse });
    }
    if bytes[5] != SUFFIX {
        return Err(MarkerError::Suffix(bytes[5]));
    }
    Ok(seq)
}
