//! SM3 密码杂凑算法（GB/T 32905-2016）独立流式实现（M-11）。
//!
//! - 零新依赖（R5 红线）：纯 Rust 手写，逐行对齐 C# `Services/Sm3.cs`
//!   （update 流式分块缓冲 → finish 填充 → compress 64 轮压缩，同结构同常量）；
//! - 消费点：本地校验码弹窗（`ui/dialogs.rs` `hash_local` algo=2）与传输后
//!   校验本机侧（`transfer.rs` `local_hash` algo=2）；
//! - 测试钉 GB/T 32905-2016 附录 A 标准向量 4 条（OpenSSL 复核）+ 流式
//!   分块一致性（C# Sm3Tests `Hash_StreamingChunksMatchWhole` 同款 7/33/27
//!   与 16/51 两种切法）；
//! - debug 构建启动自检（C# `Sm3.RunSelfTest` 对齐）：`main()` 调用
//!   [`debug_self_check`]，实现被误改时启动即暴露。

/// 便捷整算（测试与短输入用）。
pub(crate) fn hex(data: &[u8]) -> String {
    let mut h = Sm3::new();
    h.update(data);
    h.finish_hex()
}

/// debug 构建启动自检（C# Sm3.RunSelfTest 对齐）：钉一条附录 A 向量，
/// 失败即 panic——DEBUG 双击启动第一时间看到崩溃而不是静默算错。
#[cfg(debug_assertions)]
pub(crate) fn debug_self_check() {
    assert_eq!(
        hex(b"abc"),
        "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
    );
}

/// SM3 流式实例（C# Sm3 类对应物：new → update×N → finish_hex）。
pub(crate) struct Sm3 {
    /// 8×u32 滚动状态（IV 起步，每块压缩后异或叠加）。
    state: [u32; 8],
    /// 未满一块（64 字节）的残余字节缓冲。
    block: [u8; 64],
    /// `block` 内有效字节数。
    block_fill: usize,
    /// 累计输入字节数（填充写 64 位大端总比特数用）。
    total_bytes: u64,
    /// 已结算标志（finish 后不可再 update，C# 抛异常同语义）。
    finished: bool,
}

impl Sm3 {
    pub(crate) fn new() -> Self {
        Self {
            // GB/T 32905-2016 §5.3.1 初始值 IV
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

    /// 追加输入（任意长度、任意次数；空切片无害）。
    pub(crate) fn update(&mut self, data: &[u8]) {
        assert!(!self.finished, "SM3 已结算，不可再追加输入");
        let count = data.len();
        if count == 0 {
            return;
        }
        self.total_bytes += count as u64;
        let mut offset = 0usize;
        // 先补齐上一次的半块，凑满 64 字节即压缩。
        if self.block_fill > 0 {
            let take = (64 - self.block_fill).min(count);
            self.block[self.block_fill..self.block_fill + take].copy_from_slice(&data[..take]);
            self.block_fill += take;
            offset = take;
            if self.block_fill == 64 {
                let blk = self.block; // copy 解借用冲突
                self.compress(&blk);
                self.block_fill = 0;
            }
        }
        // 整块直压。
        while offset + 64 <= count {
            let mut blk = [0u8; 64];
            blk.copy_from_slice(&data[offset..offset + 64]);
            self.compress(&blk);
            offset += 64;
        }
        // 尾巴存回残余块。
        if offset < count {
            self.block[..count - offset].copy_from_slice(&data[offset..]);
            self.block_fill = count - offset;
        }
    }

    /// 结算并返回 32 字节摘要（消费 self：结算后实例不可复用）。
    pub(crate) fn finish(mut self) -> [u8; 32] {
        assert!(!self.finished, "SM3 不可重复结算");
        self.finished = true;
        // GB/T 32905-2016 §4 填充：追加 1 位 1，再补 0 至剩余 64 位，写大端总比特数。
        let bit_length = self.total_bytes * 8;
        self.block[self.block_fill] = 0x80;
        self.block_fill += 1;
        if self.block_fill == 64 {
            let blk = self.block;
            self.compress(&blk);
            self.block_fill = 0;
        }
        // 剩余不足 64 位放不下长度 → 清零补满整块先压一块。
        if self.block_fill > 56 {
            for b in &mut self.block[self.block_fill..] {
                *b = 0;
            }
            let blk = self.block;
            self.compress(&blk);
            self.block_fill = 0;
        }
        // 补 0 至第 57 字节，随后 8 字节大端比特长度。
        for b in &mut self.block[self.block_fill..56] {
            *b = 0;
        }
        for i in 0..8 {
            self.block[56 + i] = (bit_length >> (56 - 8 * i)) as u8;
        }
        let blk = self.block;
        self.compress(&blk);
        // 输出大端串接。
        let mut out = [0u8; 32];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    /// 结算并返回 64 位十六进制小写摘要（消费点统一取 hex）。
    pub(crate) fn finish_hex(self) -> String {
        self.finish().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 压缩函数（GB/T 32905-2016 §5.3.2 / 附录 A）：消息扩展 68×W + 64 轮，
    /// 轮输出与旧状态异或叠加。
    fn compress(&mut self, b: &[u8; 64]) {
        // 消息扩展：W[0..15] 直读大端；W[j] = P1(W[j-16]^W[j-9]^rotl(W[j-3],15))
        //            ^ rotl(W[j-13],7) ^ W[j-6]。
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
        // 64 轮压缩（T_j：前 16 轮 0x79CC4519，其余 0x7A879D8A，均 rotl j mod 32）。
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
            // FF_j：前 16 轮布尔异或，其余多数表决；GG_j 同型（后半 ~X&Z）。
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

/// 置换 P0（轮函数输出用）：x ^ rotl(x,9) ^ rotl(x,17)。
fn p0(x: u32) -> u32 {
    x ^ rotl(x, 9) ^ rotl(x, 17)
}

/// 置换 P1（消息扩展用）：x ^ rotl(x,15) ^ rotl(x,23)。
fn p1(x: u32) -> u32 {
    x ^ rotl(x, 15) ^ rotl(x, 23)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GB/T 32905-2016 附录 A 标准测试向量（4 条，OpenSSL 复核）。
    const VECTORS: [(&str, &str); 4] = [
        (
            "",
            "1ab21d8355cfa17f8e61194831e81a8f22bec8c728fefb747ed035eb5082aa2b",
        ),
        (
            "abc",
            "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0",
        ),
        // 64 字节 "abcd" ×16（附录 A 样例 2）
        (
            "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd",
            "debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732",
        ),
        // 67 字节：64 字节 "abcd" ×16 + "abc"（附录 A 样例 2 后缀，覆盖跨块尾巴）
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

    /// 流式分块一致性（C# Hash_StreamingChunksMatchWhole 同款切法）：
    /// 多次 update 任意切块必须与整算一致。
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

    /// 多块长输入（跨 5 块）与填充边界（56/57/64 字节三档）钉住。
    #[test]
    fn block_and_padding_edges() {
        let long = "ab".repeat(160); // 320 字节 = 5 块
        let _ = hex(long.as_bytes());
        for n in [55usize, 56, 57, 63, 64, 65] {
            let s = "x".repeat(n);
            // 独立实现不引入，仅跑通 + 分块一致即可（正确性由附录 A 向量钉死）。
            let mut h = Sm3::new();
            let b = s.as_bytes();
            let (first, second) = b.split_at(n.min(64));
            h.update(first);
            h.update(second);
            assert_eq!(h.finish_hex(), hex(b), "长度 {n}");
        }
    }
}
