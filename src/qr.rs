//! Minimal QR Code encoder (ISO/IEC 18004) for the «Загрузить с ПК» pairing
//! link: byte mode, error correction level M, versions 1–6 (up to 106 bytes;
//! the link is under 70). No dependency, so the server stays a single binary.
//! The structure follows Project Nayuki's reference implementation.

/// Largest supported version: 41×41 modules, no version information blocks.
const MAX_VERSION: usize = 6;
/// Level M, versions 1–6: error correction codewords per block and blocks.
const ECC_PER_BLOCK: [usize; MAX_VERSION] = [10, 16, 26, 18, 24, 16];
const BLOCKS: [usize; MAX_VERSION] = [1, 1, 1, 2, 2, 4];
/// Format bits of level M (L = 01, M = 00, Q = 11, H = 10).
const ECL_M: u32 = 0b00;

pub struct QrCode {
    size: usize,
    modules: Vec<bool>,
}

impl QrCode {
    /// The smallest symbol holding `data`, or `None` when it is too long.
    pub fn encode(data: &[u8]) -> Option<Self> {
        // Byte mode header: 4-bit mode + 8-bit length (versions 1–9).
        let version = (1..=MAX_VERSION).find(|&v| 12 + data.len() * 8 <= data_codewords(v) * 8)?;
        let capacity = data_codewords(version) * 8;
        let mut bits = Bits::default();
        bits.push(0b0100, 4);
        bits.push(data.len() as u32, 8);
        for &byte in data {
            bits.push(u32::from(byte), 8);
        }
        bits.push(0, (capacity - bits.len()).min(4)); // terminator
        bits.push(0, (8 - bits.len() % 8) % 8);
        let mut codewords = bits.into_bytes();
        for pad in [0xEC, 0x11].into_iter().cycle() {
            if codewords.len() * 8 >= capacity {
                break;
            }
            codewords.push(pad);
        }

        let mut symbol = Builder::new(version);
        symbol.draw_function_patterns();
        symbol.draw_codewords(&add_ecc_and_interleave(&codewords, version));
        // Any mask decodes; the one with the lowest penalty scans best.
        let mask = (0..8)
            .min_by_key(|&mask| {
                symbol.apply_mask(mask);
                symbol.draw_format_bits(mask);
                let penalty = symbol.penalty();
                symbol.apply_mask(mask); // XOR again: undo
                penalty
            })
            .expect("eight masks");
        symbol.apply_mask(mask);
        symbol.draw_format_bits(mask);
        Some(Self {
            size: symbol.size,
            modules: symbol.modules,
        })
    }

    /// Modules per side, without the quiet zone.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Whether the module at column `x`, row `y` is dark.
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }

    /// SVG path of the dark modules (one rectangle per horizontal run), for a
    /// `viewBox` of `0 0 size size` plus a quiet zone of 4 modules around it.
    pub fn svg_path(&self) -> String {
        let mut path = String::new();
        for y in 0..self.size {
            let mut x = 0;
            while x < self.size {
                if !self.get(x, y) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < self.size && self.get(x, y) {
                    x += 1;
                }
                let run = x - start;
                path.push_str(&format!("M{start} {y}h{run}v1h-{run}z"));
            }
        }
        path
    }
}

#[derive(Default)]
struct Bits(Vec<bool>);

impl Bits {
    fn push(&mut self, value: u32, count: usize) {
        for i in (0..count).rev() {
            self.0.push((value >> i) & 1 != 0);
        }
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn into_bytes(self) -> Vec<u8> {
        self.0
            .chunks(8)
            .map(|chunk| {
                chunk
                    .iter()
                    .enumerate()
                    .fold(0u8, |byte, (i, &bit)| byte | (u8::from(bit) << (7 - i)))
            })
            .collect()
    }
}

/// Modules left for data and error correction after the function patterns.
fn raw_data_modules(version: usize) -> usize {
    let mut result = (16 * version + 128) * version + 64;
    if version >= 2 {
        let align = version / 7 + 2;
        result -= (25 * align - 10) * align - 55;
        if version >= 7 {
            result -= 36;
        }
    }
    result
}

fn data_codewords(version: usize) -> usize {
    raw_data_modules(version) / 8 - ECC_PER_BLOCK[version - 1] * BLOCKS[version - 1]
}

/// Split into blocks, append Reed–Solomon codewords, interleave the blocks.
fn add_ecc_and_interleave(data: &[u8], version: usize) -> Vec<u8> {
    let blocks = BLOCKS[version - 1];
    let ecc_len = ECC_PER_BLOCK[version - 1];
    let raw = raw_data_modules(version) / 8;
    let short_blocks = blocks - raw % blocks;
    let short_len = raw / blocks;
    let divisor = rs_divisor(ecc_len);
    let mut split = Vec::with_capacity(blocks);
    let mut start = 0;
    for i in 0..blocks {
        let len = short_len - ecc_len + usize::from(i >= short_blocks);
        let chunk = &data[start..start + len];
        start += len;
        let mut block = chunk.to_vec();
        if i < short_blocks {
            block.push(0); // placeholder: every block gets the same length
        }
        block.extend(rs_remainder(chunk, &divisor));
        split.push(block);
    }
    let mut result = Vec::with_capacity(raw);
    for i in 0..split[0].len() {
        for (j, block) in split.iter().enumerate() {
            if i != short_len - ecc_len || j >= short_blocks {
                result.push(block[i]);
            }
        }
    }
    result
}

/// Multiplication in GF(2^8) modulo x^8 + x^4 + x^3 + x^2 + 1.
fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z = 0u8;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x1D);
        z ^= ((y >> i) & 1) * x;
    }
    z
}

/// Generator polynomial of the given degree, highest coefficient omitted.
fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 0x02);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &byte in data {
        let factor = byte ^ result.remove(0);
        result.push(0);
        for (r, &d) in result.iter_mut().zip(divisor) {
            *r ^= gf_mul(d, factor);
        }
    }
    result
}

struct Builder {
    version: usize,
    size: usize,
    modules: Vec<bool>,
    function: Vec<bool>,
}

impl Builder {
    fn new(version: usize) -> Self {
        let size = version * 4 + 17;
        Self {
            version,
            size,
            modules: vec![false; size * size],
            function: vec![false; size * size],
        }
    }

    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }

    fn draw_function_patterns(&mut self) {
        for i in 0..self.size {
            self.set_function(6, i, i % 2 == 0);
            self.set_function(i, 6, i % 2 == 0);
        }
        let far = self.size as i32 - 4;
        self.draw_finder(3, 3);
        self.draw_finder(far, 3);
        self.draw_finder(3, far);
        // Versions 2–6 have one alignment pattern, in the bottom-right corner.
        if self.version >= 2 {
            let at = self.size as i32 - 7;
            for dy in -2..=2i32 {
                for dx in -2..=2i32 {
                    let dark = dx.abs().max(dy.abs()) != 1;
                    self.set_function((at + dx) as usize, (at + dy) as usize, dark);
                }
            }
        }
        self.draw_format_bits(0); // reserve the area; redrawn with the mask
    }

    /// Finder pattern centred at (x, y) with its light separator.
    fn draw_finder(&mut self, x: i32, y: i32) {
        for dy in -4..=4i32 {
            for dx in -4..=4i32 {
                let (xx, yy) = (x + dx, y + dy);
                if (0..self.size as i32).contains(&xx) && (0..self.size as i32).contains(&yy) {
                    let dist = dx.abs().max(dy.abs());
                    self.set_function(xx as usize, yy as usize, dist != 2 && dist != 4);
                }
            }
        }
    }

    /// Error correction level and mask, BCH(15,5)-protected, in both copies.
    fn draw_format_bits(&mut self, mask: u32) {
        let data = (ECL_M << 3) | mask;
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let bit = |i: u32| (bits >> i) & 1 != 0;
        let size = self.size;
        for i in 0..=5 {
            self.set_function(8, i, bit(i as u32));
        }
        self.set_function(8, 7, bit(6));
        self.set_function(8, 8, bit(7));
        self.set_function(7, 8, bit(8));
        for i in 9..15 {
            self.set_function(14 - i, 8, bit(i as u32));
        }
        for i in 0..8 {
            self.set_function(size - 1 - i, 8, bit(i as u32));
        }
        for i in 8..15 {
            self.set_function(8, size - 15 + i, bit(i as u32));
        }
        self.set_function(8, size - 8, true); // the dark module
    }

    /// Codeword bits in the zigzag order: two-column strips from the right,
    /// alternately upwards and downwards, skipping the vertical timing line.
    fn draw_codewords(&mut self, data: &[u8]) {
        let size = self.size;
        let mut i = 0;
        let mut right = size - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..size {
                for j in 0..2 {
                    let x = right - j;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward { size - 1 - vert } else { vert };
                    if !self.function[y * size + x] && i < data.len() * 8 {
                        self.modules[y * size + x] = (data[i >> 3] >> (7 - (i & 7))) & 1 != 0;
                        i += 1;
                    }
                }
            }
            if right < 2 {
                break;
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: u32) {
        for y in 0..self.size {
            for x in 0..self.size {
                if !self.function[y * self.size + x] && mask_bit(mask, x, y) {
                    self.modules[y * self.size + x] ^= true;
                }
            }
        }
    }

    fn get(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }

    /// Simplified ISO penalty: long runs, 2×2 blocks, finder-like patterns
    /// and dark/light balance.
    fn penalty(&self) -> u32 {
        let size = self.size;
        let mut score = 0u32;
        for horizontal in [true, false] {
            for a in 0..size {
                let line: Vec<bool> = (0..size)
                    .map(|b| {
                        if horizontal {
                            self.get(b, a)
                        } else {
                            self.get(a, b)
                        }
                    })
                    .collect();
                let mut run = 1;
                for b in 1..=size {
                    if b < size && line[b] == line[b - 1] {
                        run += 1;
                        continue;
                    }
                    if run >= 5 {
                        score += 3 + (run - 5) as u32;
                    }
                    run = 1;
                }
                // 1:1:3:1:1 with four light modules on one side (outside is light).
                let at = |i: i32| i >= 0 && (i as usize) < size && line[i as usize];
                for start in -4..size as i32 {
                    let core = [true, false, true, true, true, false, true];
                    if (0..7).all(|k| at(start + k) == core[k as usize]) {
                        let before = (1..=4).all(|k| !at(start - k));
                        let after = (7..11).all(|k| !at(start + k));
                        if before || after {
                            score += 40;
                        }
                    }
                }
            }
        }
        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let c = self.get(x, y);
                if c == self.get(x + 1, y) && c == self.get(x, y + 1) && c == self.get(x + 1, y + 1)
                {
                    score += 3;
                }
            }
        }
        let dark = self.modules.iter().filter(|&&m| m).count();
        let percent = dark * 100 / self.modules.len();
        score += (percent.abs_diff(50) / 5) as u32 * 10;
        score
    }
}

fn mask_bit(mask: u32, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y).is_multiple_of(2),
        1 => y.is_multiple_of(2),
        2 => x.is_multiple_of(3),
        3 => (x + y).is_multiple_of(3),
        4 => (x / 3 + y / 2).is_multiple_of(2),
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3).is_multiple_of(2),
        _ => ((x + y) % 2 + x * y % 3).is_multiple_of(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reed_solomon_matches_the_hello_world_example() {
        // «HELLO WORLD», version 1-M (thonky.com QR tutorial).
        let data = [
            32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17,
        ];
        assert_eq!(
            rs_remainder(&data, &rs_divisor(10)),
            [196, 35, 39, 119, 235, 215, 231, 226, 93, 23]
        );
    }

    #[test]
    fn capacities_match_the_standard() {
        let bytes: Vec<usize> = (1..=MAX_VERSION).map(|v| data_codewords(v) - 2).collect();
        assert_eq!(bytes, [14, 26, 42, 62, 84, 106]);
        assert!(QrCode::encode(&[b'x'; 106]).is_some());
        assert!(QrCode::encode(&[b'x'; 107]).is_none());
    }

    /// Format information of level M from the standard's table, masks 0–7.
    const FORMAT_M: [&str; 8] = [
        "101010000010010",
        "101000100100101",
        "101111001111100",
        "101101101001011",
        "100010111111001",
        "100000011001110",
        "100111110010111",
        "100101010100000",
    ];

    // --- an independent reader of the generated module grid ---------------

    fn exp_table() -> ([u8; 256], [u8; 256]) {
        let (mut exp, mut log) = ([0u8; 256], [0u8; 256]);
        let mut x = 1u16;
        for (i, e) in exp.iter_mut().enumerate().take(255) {
            *e = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= 0x11D;
            }
        }
        (exp, log)
    }

    /// Syndromes of a received block: all zero for a valid codeword.
    fn syndromes_are_zero(block: &[u8], ecc_len: usize) -> bool {
        let (exp, log) = exp_table();
        (0..ecc_len).all(|i| {
            block.iter().fold(0u8, |acc, &c| {
                let shifted = if acc == 0 {
                    0
                } else {
                    exp[(log[acc as usize] as usize + i) % 255]
                };
                shifted ^ c
            }) == 0
        })
    }

    fn is_function(size: usize, version: usize, row: usize, col: usize) -> bool {
        let finder =
            |r0: usize, c0: usize| (r0..r0 + 8).contains(&row) && (c0..c0 + 8).contains(&col);
        let near = size - 8;
        let align = version >= 2
            && (size - 9..=size - 5).contains(&row)
            && (size - 9..=size - 5).contains(&col);
        finder(0, 0)
            || finder(0, near)
            || finder(near, 0)
            || row == 6
            || col == 6
            || align
            || (row == 8 && (col <= 8 || col >= near))
            || (col == 8 && (row <= 8 || row >= near))
    }

    /// Read back the payload: format bits, unmasking, zigzag, blocks with RS
    /// syndromes, byte segment and padding.
    fn decode(qr: &QrCode) -> Vec<u8> {
        let size = qr.size();
        let version = (size - 17) / 4;
        let module = |row: usize, col: usize| qr.get(col, row);

        // Format bits, MSB first: row 8 left to right, then column 8 upwards.
        let mut cells: Vec<(usize, usize)> = (0..=8).filter(|&c| c != 6).map(|c| (8, c)).collect();
        cells.extend((0..=7).rev().filter(|&r| r != 6).map(|r| (r, 8)));
        let format: String = cells
            .iter()
            .map(|&(r, c)| if module(r, c) { '1' } else { '0' })
            .collect();
        let mask = FORMAT_M
            .iter()
            .position(|f| *f == format)
            .unwrap_or_else(|| panic!("unknown format bits {format}")) as u32;
        // The second copy: column 8 bottom-up (bits 14..8), then row 8 (7..0).
        let mut second: String = (size - 7..size)
            .rev()
            .map(|r| if module(r, 8) { '1' } else { '0' })
            .collect();
        second.extend((size - 8..size).map(|c| if module(8, c) { '1' } else { '0' }));
        assert_eq!(second, format, "both format copies agree");
        assert!(module(size - 8, 8), "dark module");

        let mut bits = Vec::new();
        let mut col = size - 1;
        let mut upward = true;
        loop {
            if col == 6 {
                col = 5;
            }
            for step in 0..size {
                let row = if upward { size - 1 - step } else { step };
                for c in [col, col - 1] {
                    if !is_function(size, version, row, c) {
                        let flip = mask_bit(mask, c, row);
                        bits.push(module(row, c) ^ flip);
                    }
                }
            }
            upward = !upward;
            if col < 3 {
                break;
            }
            col -= 2;
        }
        let raw = raw_data_modules(version) / 8;
        assert_eq!(bits.len(), raw_data_modules(version), "data modules");
        let codewords: Vec<u8> = bits[..raw * 8]
            .chunks(8)
            .map(|b| b.iter().fold(0, |acc, &bit| (acc << 1) | u8::from(bit)))
            .collect();

        let (blocks, ecc_len) = (BLOCKS[version - 1], ECC_PER_BLOCK[version - 1]);
        assert_eq!(raw % blocks, 0, "level M up to version 6 has equal blocks");
        let block_len = raw / blocks;
        let data_len = block_len - ecc_len;
        let mut payload = Vec::new();
        let mut split = vec![Vec::new(); blocks];
        for (i, &cw) in codewords.iter().enumerate() {
            split[i % blocks].push(cw);
        }
        for block in &split {
            assert!(syndromes_are_zero(block, ecc_len), "RS syndromes");
        }
        for block in &split {
            payload.extend_from_slice(&block[..data_len]);
        }

        let bit = |i: usize| (payload[i / 8] >> (7 - i % 8)) & 1;
        let read = |from: usize, n: usize| {
            (from..from + n).fold(0usize, |a, i| (a << 1) | bit(i) as usize)
        };
        assert_eq!(read(0, 4), 0b0100, "byte mode");
        let len = read(4, 8);
        let out: Vec<u8> = (0..len).map(|i| read(12 + i * 8, 8) as u8).collect();
        let end = 12 + len * 8;
        let terminator = (payload.len() * 8 - end).min(4);
        assert_eq!(read(end, terminator), 0, "terminator");
        let padded = (end + terminator).div_ceil(8);
        for (i, &byte) in payload[padded..].iter().enumerate() {
            assert_eq!(byte, if i % 2 == 0 { 0xEC } else { 0x11 }, "pad bytes");
        }
        out
    }

    #[test]
    fn format_bits_match_the_standard_table() {
        for (mask, expected) in FORMAT_M.iter().enumerate() {
            let mut b = Builder::new(1);
            b.draw_format_bits(mask as u32);
            let mut cells: Vec<(usize, usize)> =
                (0..=8).filter(|&c| c != 6).map(|c| (8, c)).collect();
            cells.extend((0..=7).rev().filter(|&r| r != 6).map(|r| (r, 8)));
            let got: String = cells
                .iter()
                .map(|&(r, c)| if b.get(c, r) { '1' } else { '0' })
                .collect();
            assert_eq!(&got, expected, "mask {mask}");
        }
    }

    #[test]
    fn symbols_decode_back_for_every_version() {
        let link = "https://t.me/files_test_bot?startapp=pc_ABCD2345";
        let samples: Vec<Vec<u8>> = vec![
            b"pc".to_vec(),
            b"https://t.me/abc?startapp=x".to_vec(),
            link.as_bytes().to_vec(),
            format!("https://t.me/{}?startapp=pc_ABCD2345", "b".repeat(32)).into_bytes(),
            "Привет, файлы".as_bytes().to_vec(),
            vec![b'x'; 106],
        ];
        let mut versions = Vec::new();
        for data in samples {
            let qr = QrCode::encode(&data).unwrap();
            versions.push((qr.size() - 17) / 4);
            assert_eq!(decode(&qr), data);
        }
        assert_eq!(versions, [1, 3, 4, 5, 2, 6]);
    }

    #[test]
    fn every_mask_decodes() {
        // Force each mask through the same pipeline the encoder uses.
        let data = b"https://t.me/files_test_bot?startapp=pc_ABCD2345";
        let qr = QrCode::encode(data).unwrap();
        let version = (qr.size() - 17) / 4;
        let mut codewords = Vec::new();
        {
            let capacity = data_codewords(version) * 8;
            let mut bits = Bits::default();
            bits.push(0b0100, 4);
            bits.push(data.len() as u32, 8);
            for &b in data.iter() {
                bits.push(u32::from(b), 8);
            }
            bits.push(0, (capacity - bits.len()).min(4));
            bits.push(0, (8 - bits.len() % 8) % 8);
            codewords.extend(bits.into_bytes());
            while codewords.len() * 8 < capacity {
                codewords.push(if codewords.len() % 2 == 0 { 0xEC } else { 0x11 });
            }
        }
        for mask in 0..8 {
            let mut b = Builder::new(version);
            b.draw_function_patterns();
            b.draw_codewords(&add_ecc_and_interleave(&codewords, version));
            b.apply_mask(mask);
            b.draw_format_bits(mask);
            let qr = QrCode {
                size: b.size,
                modules: b.modules,
            };
            assert_eq!(decode(&qr), data, "mask {mask}");
        }
    }

    #[test]
    fn finder_patterns_and_svg() {
        let qr = QrCode::encode(b"pc").unwrap();
        assert_eq!(qr.size(), 21);
        // Top-left finder: dark border, light ring, dark core; light separator.
        let row: Vec<bool> = (0..8).map(|x| qr.get(x, 0)).collect();
        assert_eq!(row, [true, true, true, true, true, true, true, false]);
        let row: Vec<bool> = (0..8).map(|x| qr.get(x, 2)).collect();
        assert_eq!(row, [true, false, true, true, true, false, true, false]);
        // Timing pattern between the finders.
        let timing: Vec<bool> = (8..13).map(|x| qr.get(x, 6)).collect();
        assert_eq!(timing, [true, false, true, false, true]);
        let path = qr.svg_path();
        assert!(path.starts_with("M0 0h7v1h-7z"), "{path}");
        assert!(path
            .chars()
            .all(|c| c.is_ascii_digit() || "Mhvz -".contains(c)));
    }
}
