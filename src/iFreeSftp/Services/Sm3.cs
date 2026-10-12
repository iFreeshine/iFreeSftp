using System;

namespace iFreeSftp.Services;

/// <summary>
/// SM3 密码杂凑算法（GB/T 32905-2016 / IETF draft-sca-csm-sm3）的托管流式实现。
/// .NET BCL 与 Windows 桌面版 CNG 均未内置 SM3（服务器 GM TLS 才有），故自带实现。
/// 用法对齐 BCL IncrementalHash：分块 AppendData 后 GetHashHex。
/// 非线程安全，由调用方（SftpSessionService._sshLock 串行计算）保证。
/// </summary>
public sealed class Sm3
{
    // 压缩函数轮常数: T_j
    private const uint T0To15 = 0x79CC4519u;
    private const uint T16To63 = 0x7A879D8Au;

    // 初始值 IV
    private readonly uint[] _state =
    {
        0x7380166Fu, 0x4914B2B9u, 0x172442D7u, 0xDA8A0600u,
        0xA96F30BCu, 0x163138AAu, 0xE38DEE4Du, 0xB0FB0E4Eu,
    };

    private readonly byte[] _block = new byte[64];
    private int _blockFill;
    private ulong _totalBytes;
    private bool _finished;

    /// <summary>追加一段数据（buffer 前 count 字节）。</summary>
    public void AppendData(byte[] buffer, int count)
    {
        if (_finished)
        {
            throw new InvalidOperationException("SM3 已结束, 不可继续追加");
        }

        if (count <= 0)
        {
            return;
        }

        _totalBytes += (ulong)count;
        int offset = 0;
        if (_blockFill > 0)
        {
            int take = Math.Min(64 - _blockFill, count);
            Buffer.BlockCopy(buffer, 0, _block, _blockFill, take);
            _blockFill += take;
            offset = take;
            if (_blockFill == 64)
            {
                Compress(_block, 0);
                _blockFill = 0;
            }
        }

        while (offset + 64 <= count)
        {
            Compress(buffer, offset);
            offset += 64;
        }

        if (offset < count)
        {
            Buffer.BlockCopy(buffer, offset, _block, 0, count - offset);
            _blockFill = count - offset;
        }
    }

    /// <summary>结束哈希并返回小写十六进制摘要。对象此后不可再用（复用请重新 new）。</summary>
    public string GetHashHex()
    {
        var digest = new byte[32];
        Finish(digest);
        return Convert.ToHexString(digest).ToLowerInvariant();
    }

    /// <summary>填充（0x80 + 0 填充 + 64 位大端比特长度）并输出 32 字节摘要。</summary>
    private void Finish(byte[] output)
    {
        if (_finished)
        {
            throw new InvalidOperationException("SM3 已结束");
        }

        ulong bitLength = _totalBytes * 8UL;

        // 1) 追加 0x80
        _block[_blockFill++] = 0x80;
        if (_blockFill == 64)
        {
            Compress(_block, 0);
            _blockFill = 0;
        }

        // 2) 长度放不进当前块（剩余 < 8 字节）时先补满一整块
        if (_blockFill > 56)
        {
            Array.Clear(_block, _blockFill, 64 - _blockFill);
            Compress(_block, 0);
            _blockFill = 0;
        }

        // 3) 补 0 至第 56 字节, 写入 64 位大端比特长度后压缩
        Array.Clear(_block, _blockFill, 56 - _blockFill);
        for (int i = 0; i < 8; i++)
        {
            _block[56 + i] = (byte)(bitLength >> (56 - (8 * i)));
        }

        Compress(_block, 0);
        _blockFill = 0;
        _finished = true;

        for (int i = 0; i < 8; i++)
        {
            uint s = _state[i];
            output[(4 * i) + 0] = (byte)(s >> 24);
            output[(4 * i) + 1] = (byte)(s >> 16);
            output[(4 * i) + 2] = (byte)(s >> 8);
            output[(4 * i) + 3] = (byte)s;
        }
    }

    /// <summary>对单个 64 字节块执行 64 轮压缩并异或进状态。</summary>
    private void Compress(byte[] b, int off)
    {
        Span<uint> w = stackalloc uint[68];
        Span<uint> w1 = stackalloc uint[64];
        for (int i = 0; i < 16; i++)
        {
            int o = off + (4 * i);
            w[i] = ((uint)b[o] << 24) | ((uint)b[o + 1] << 16) | ((uint)b[o + 2] << 8) | b[o + 3];
        }

        for (int j = 16; j < 68; j++)
        {
            w[j] = P1(w[j - 16] ^ w[j - 9] ^ Rotl(w[j - 3], 15)) ^ Rotl(w[j - 13], 7) ^ w[j - 6];
        }

        for (int j = 0; j < 64; j++)
        {
            w1[j] = w[j] ^ w[j + 4];
        }

        uint a = _state[0], b2 = _state[1], c = _state[2], d = _state[3];
        uint e = _state[4], f = _state[5], g = _state[6], h = _state[7];

        for (int j = 0; j < 64; j++)
        {
            uint t = j < 16 ? T0To15 : T16To63;
            uint ss1 = Rotl(Rotl(a, 12) + e + Rotl(t, j % 32), 7);
            uint ss2 = ss1 ^ Rotl(a, 12);
            uint ff = j < 16 ? a ^ b2 ^ c : (a & b2) | (a & c) | (b2 & c);
            uint gg = j < 16 ? e ^ f ^ g : (e & f) | (~e & g);
            uint tt1 = ff + d + ss2 + w1[j];
            uint tt2 = gg + h + ss1 + w[j];
            d = c;
            c = Rotl(b2, 9);
            b2 = a;
            a = tt1;
            h = g;
            g = Rotl(f, 19);
            f = e;
            e = P0(tt2);
        }

        _state[0] ^= a;
        _state[1] ^= b2;
        _state[2] ^= c;
        _state[3] ^= d;
        _state[4] ^= e;
        _state[5] ^= f;
        _state[6] ^= g;
        _state[7] ^= h;
    }

    private static uint Rotl(uint x, int n) => (x << n) | (x >> (32 - n));

    private static uint P0(uint x) => x ^ Rotl(x, 9) ^ Rotl(x, 17);

    private static uint P1(uint x) => x ^ Rotl(x, 15) ^ Rotl(x, 23);

#if DEBUG
    /// <summary>
    /// GB/T 32905-2016 附录 A 标准向量自测（仅 DEBUG 启动跑一次；向量值经 OpenSSL
    /// 独立实现复核）。覆盖三条压缩路径：单块短消息、整 64B 块、67B 跨块收尾；
    /// 并验证流式分块追加与整算一致。失败抛异常拦截启动（自实现被误改时立即暴露）。
    /// </summary>
    public static void RunSelfTest()
    {
        (byte[] Data, string Expected)[] vectors =
        {
            ("abc"u8.ToArray(), "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"),
            ("abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd"u8.ToArray(),
             "debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732"),
            ("abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabc"u8.ToArray(),
             "952eb84cacee9c10bde4d6882d29d63140ba72af6fe485085095dccd5b872453"),
        };

        foreach (var (data, expected) in vectors)
        {
            var sm3 = new Sm3();
            sm3.AppendData(data, data.Length);
            AssertHex(sm3.GetHashHex(), expected, $"整算 ({data.Length}B)");
        }

        // 流式分块: 向量 3 拆两次追加 (16B + 剩余 51B), 验证跨块缓冲与整算一致
        var streamed = new Sm3();
        streamed.AppendData(vectors[2].Data, 16);
        var rest = vectors[2].Data.AsSpan(16).ToArray();
        streamed.AppendData(rest, rest.Length);
        AssertHex(streamed.GetHashHex(), vectors[2].Expected, "流式分块");

        static void AssertHex(string actual, string expected, string label)
        {
            if (actual != expected)
            {
                throw new InvalidOperationException(
                    $"SM3 标准向量自测失败 [{label}]: 期望 {expected}, 实得 {actual}");
            }
        }
    }
#endif
}
