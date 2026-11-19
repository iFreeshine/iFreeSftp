pub(crate) fn hex(data: &[u8]) -> String {
    let mut h = Sm3::new();
    h.update(data);
    h.finish_hex()
}

#[cfg(debug_assertions)]
pub(crate) fn debug_self_check() {
    assert_eq!(
        hex(b"abc"),
        "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
    );
}

pub(crate) struct Sm3 {

    state: [u32; 8],

    block: [u8; 64],

    block_fill: usize,

    total_bytes: u64,

    finished: bool,
}

impl Sm3 {
    pub(crate) fn new() -> Self {
        Self {

            state: [
                0x7380166f, 0x4914b2b9, 0x172442d7, 0xda8a0600, 0xa96f30bc, 0x163138aa, 0xe38dee4d,
                0xb0fb0e4e,
            ],
            block: [0u8; 64],
            block_fill: 0,
            total_bytes: 0,
            finished: false,
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        assert!(!self.finished, "SM3 已结算，不可再追加输入");
        let count = data.len();
        if count == 0 {
            return;
        }
        self.total_bytes += count as u64;
        let mut offset = 0usize;

        if self.block_fill > 0 {
            let take = (64 - self.block_fill).min(count);
            self.block[self.block_fill..self.block_fill + take].copy_from_slice(&data[..take]);
            self.block_fill += take;
            offset = take;
            if self.block_fill == 64 {
                let blk = self.block;
                self.compress(&blk);
                self.block_fill = 0;
            }
        }

        while offset + 64 <= count {
            let mut blk = [0u8; 64];
            blk.copy_from_slice(&data[offset..offset + 64]);
            self.compress(&blk);
            offset += 64;
        }

        if offset < count {
            self.block[..count - offset].copy_from_slice(&data[offset..]);
            self.block_fill = count - offset;
        }
    }

    pub(crate) fn finish(mut self) -> [u8; 32] {
        assert!(!self.finished, "SM3 不可重复结算");
        self.finished = true;

        let bit_length = self.total_bytes * 8;
        self.block[self.block_fill] = 0x80;
        self.block_fill += 1;
        if self.block_fill == 64 {
            let blk = self.block;
            self.compress(&blk);
            self.block_fill = 0;
        }

        if self.block_fill > 56 {
            for b in &mut self.block[self.block_fill..] {
                *b = 0;
            }
            let blk = self.block;
            self.compress(&blk);
            self.block_fill = 0;
        }

        for b in &mut self.block[self.block_fill..56] {
            *b = 0;
        }
        for i in 0..8 {
            self.block[56 + i] = (bit_length >> (56 - 8 * i)) as u8;
        }
        let blk = self.block;
        self.compress(&blk);

        let mut out = [0u8; 32];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    pub(crate) fn finish_hex(self) -> String {
        self.finish().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn compress(&mut self, b: &[u8; 64]) {

        let mut w = [0u32; 68];
        let mut w1 = [0u32; 64];
        for i in 0..16 {
            let o = 4 * i;
            w[i] = u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        }
        for j in 16..68 {
            w[j] = p1(w[j - 16] ^ w[j - 9] ^ rotl(w[j - 3], 15)) ^ rotl(w[j - 13], 7) ^ w[j - 6];
        }
        for j in 0..64 {
            w1[j] = w[j] ^ w[j + 4];
        }

        let (mut a, mut b2, mut c, mut d) =
            (self.state[0], self.state[1], self.state[2], self.state[3]);
        let (mut e, mut f, mut g, mut h) =
            (self.state[4], self.state[5], self.state[6], self.state[7]);
        for j in 0..64 {
            let t = if j < 16 { 0x79CC4519u32 } else { 0x7A879D8A };
            let ss1 = rotl(
                rotl(a, 12)
                    .wrapping_add(e)
                    .wrapping_add(rotl(t, (j % 32) as u32)),
                7,
            );
            let ss2 = ss1 ^ rotl(a, 12);

            let ff = if j < 16 {
                a ^ b2 ^ c
            } else {
                (a & b2) | (a & c) | (b2 & c)
            };
            let gg = if j < 16 {
                e ^ f ^ g
            } else {
                (e & f) | (!e & g)
            };
            let tt1 = ff.wrapping_add(d).wrapping_add(ss2).wrapping_add(w1[j]);
            let tt2 = gg.wrapping_add(h).wrapping_add(ss1).wrapping_add(w[j]);
            d = c;
            c = rotl(b2, 9);
            b2 = a;
            a = tt1;
            h = g;
            g = rotl(f, 19);
            f = e;
            e = p0(tt2);
        }
        let rounds = [a, b2, c, d, e, f, g, h];
        for i in 0..8 {
            self.state[i] ^= rounds[i];
        }
    }
}

fn rotl(x: u32, n: u32) -> u32 {
    x.rotate_left(n)
}

fn p0(x: u32) -> u32 {
    x ^ rotl(x, 9) ^ rotl(x, 17)
}

fn p1(x: u32) -> u32 {
    x ^ rotl(x, 15) ^ rotl(x, 23)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: [(&str, &str); 4] = [
        (
            "",
            "1ab21d8355cfa17f8e61194831e81a8f22bec8c728fefb747ed035eb5082aa2b",
        ),
        (
            "abc",
            "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0",
        ),

        (
            "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd",
            "debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732",
        ),

        (
            "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabc",
            "952eb84cacee9c10bde4d6882d29d63140ba72af6fe485085095dccd5b872453",
        ),
    ];

    #[test]
    fn gbt32905_2016_appendix_a_vectors() {
        for (input, expect) in VECTORS {
            assert_eq!(hex(input.as_bytes()), expect, "输入 {input:?}");
        }
    }

    #[test]
    fn streaming_chunks_match_whole() {
        let whole = "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabc";
        let cut_list: [&[usize]; 4] = [&[7, 33, 27], &[16, 51], &[1, 1, 65], &[64, 3]];
        for cuts in cut_list {
            let mut h = Sm3::new();
            let mut rest = whole;
            for &c in cuts {
                h.update(rest[..c].as_bytes());
                rest = &rest[c..];
            }
            h.update(rest.as_bytes());
            assert_eq!(h.finish_hex(), hex(whole.as_bytes()), "切块 {cuts:?}");
        }
    }

    #[test]
    fn block_and_padding_edges() {
        let long = "ab".repeat(160);
        let _ = hex(long.as_bytes());
        for n in [55usize, 56, 57, 63, 64, 65] {
            let s = "x".repeat(n);

            let mut h = Sm3::new();
            let b = s.as_bytes();
            let (first, second) = b.split_at(n.min(64));
            h.update(first);
            h.update(second);
            assert_eq!(h.finish_hex(), hex(b), "长度 {n}");
        }
    }
}
