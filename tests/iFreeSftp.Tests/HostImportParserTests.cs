using System.Text;
using System.Text.Json;
using System.Xml;
using iFreeSftp.Models;
using iFreeSftp.Services;
using Xunit;

namespace iFreeSftp.Tests;

// =============================================================================
// HostImportParser 单测：全部使用合成数据（内联字符串），不触碰真实注册表与
// 用户文件。红线断言（口令/命令字段绝不出现）贯穿各来源用例。
// =============================================================================

public class HostImportParserTests
{
    private static ImportSourceStat NewStat(ImportSourceKind kind) => new() { Kind = kind };

    /// <summary>红线断言：机密串不出现在任何导入字符串字段中。</summary>
    private static void AssertNoSecret(List<ImportedSession> sessions, params string[] secrets)
    {
        foreach (var s in sessions)
        {
            foreach (var field in new[]
                { s.Name, s.Host, s.Username, s.KeyFilePath, s.OwnStorePassword, s.Warning })
            {
                foreach (var secret in secrets)
                    Assert.DoesNotContain(secret, field, StringComparison.OrdinalIgnoreCase);
            }
        }
    }

    // =========================================================================
    // DecodeBytes：编码探测链
    // =========================================================================

    public class DecodeBytesTests
    {
        [Fact]
        public void Utf8Bom_ChineseText()
        {
            var text = "主机配置";
            var bytes = new byte[] { 0xEF, 0xBB, 0xBF }.Concat(Encoding.UTF8.GetBytes(text)).ToArray();
            Assert.Equal(text, HostImportParser.DecodeBytes(bytes));
        }

        [Fact]
        public void Utf16LeBom_WinScpIniDefault()
        {
            var text = "[Sessions\\prod]\r\nHostName=10.0.0.5\r\n";
            var bytes = new byte[] { 0xFF, 0xFE }.Concat(Encoding.Unicode.GetBytes(text)).ToArray();
            Assert.Equal(text, HostImportParser.DecodeBytes(bytes));
        }

        [Fact]
        public void Utf16BeBom()
        {
            var text = "HostName=example.com";
            var bytes = new byte[] { 0xFE, 0xFF }.Concat(Encoding.BigEndianUnicode.GetBytes(text)).ToArray();
            Assert.Equal(text, HostImportParser.DecodeBytes(bytes));
        }

        [Fact]
        public void PlainUtf8_NoBom()
        {
            var bytes = Encoding.UTF8.GetBytes("HostName=example.com");
            Assert.Equal("HostName=example.com", HostImportParser.DecodeBytes(bytes));
        }

        [Fact]
        public void InvalidUtf8_FallsBackToAnsi()
        {
            // 触发 HostImportParser 静态构造器（注册 CodePages 提供器）
            Assert.Equal(string.Empty, HostImportParser.DecodeBytes(Array.Empty<byte>()));
            // 0xFF 在严格 UTF-8 下非法 → 回退系统 ANSI；断言与 ANSI 解码一致（跨机器成立）
            var bytes = new byte[] { 0xFF, 0x41, 0x42 };
            Assert.Equal(Encoding.GetEncoding(0).GetString(bytes), HostImportParser.DecodeBytes(bytes));
        }
    }

    // =========================================================================
    // WinSCP INI
    // =========================================================================

    public class WinScpIniTests
    {
        private const string SampleIni =
            "[Configuration\\Interface]\n" +
            "ShowWelcomeDialog=1\n" +
            "\n" +
            "[Sessions\\prod%20server]\n" +
            "HostName=10.0.0.5\n" +
            "PortNumber=2200\n" +
            "UserName=ops\n" +
            "PublicKeyFile=C:\\keys\\id_rsa.pub\n" +
            "Protocol=SFTP\n" +
            "Password=winscp-obfuscated-secret\n" +
            "RemoteCommand=ls -la /var/www\n" +
            "\n" +
            "[Sessions\\work\\dev]\n" +
            "HostName=dev.example.com\n" +
            "PortNumber=22\n" +
            "UserName=dev\n" +
            "Protocol=SCP\n" +
            "\n" +
            "[Sessions\\legacy]\n" +
            "HostName=old.example.com\n";

        [Fact]
        public void ParsesSessions_FolderAndEscapedNames()
        {
            var stat = NewStat(ImportSourceKind.WinScpIni);
            var sessions = HostImportParser.ParseWinScpIni(SampleIni, stat);

            Assert.Equal(2, sessions.Count);
            Assert.Equal(3, stat.Found);
            Assert.Equal(1, stat.SkippedNonSftp);

            var prod = sessions[0];
            Assert.Equal("prod server", prod.Name);       // %20 还原
            Assert.Equal("10.0.0.5", prod.Host);
            Assert.Equal(2200, prod.Port);
            Assert.Equal("ops", prod.Username);
            Assert.Equal(AuthMode.KeyFile, prod.AuthMode);
            Assert.Equal(@"C:\keys\id_rsa.pub", prod.KeyFilePath);

            var legacy = sessions[1];
            Assert.Equal("old.example.com", legacy.Host);
            Assert.Equal(22, legacy.Port);                 // 缺省端口
            Assert.Equal(AuthMode.Password, legacy.AuthMode);
        }

        [Fact]
        public void RedLines_NeverImportsPasswordOrRemoteCommand()
        {
            var stat = NewStat(ImportSourceKind.WinScpIni);
            var sessions = HostImportParser.ParseWinScpIni(SampleIni, stat);
            AssertNoSecret(sessions, "winscp-obfuscated-secret", "ls -la");
            Assert.Equal(string.Empty, sessions[0].OwnStorePassword);
        }

        [Fact]
        public void Utf16LeEncodedFile_DecodesAndParses()
        {
            var bytes = new byte[] { 0xFF, 0xFE }
                .Concat(Encoding.Unicode.GetBytes(SampleIni)).ToArray();
            var text = HostImportParser.DecodeBytes(bytes);
            var stat = NewStat(ImportSourceKind.WinScpIni);
            Assert.Equal(2, HostImportParser.ParseWinScpIni(text, stat).Count);
        }

        [Fact]
        public void InvalidPort_SkipsSession()
        {
            var ini = "[Sessions\\bad]\nHostName=h\nPortNumber=70000\n";
            var stat = NewStat(ImportSourceKind.WinScpIni);
            Assert.Empty(HostImportParser.ParseWinScpIni(ini, stat));
            Assert.Equal(1, stat.SkippedInvalid);
        }
    }

    // =========================================================================
    // FileZilla sitemanager.xml
    // =========================================================================

    public class FileZillaTests
    {
        private const string SampleXml =
            """
            <?xml version="1.0" encoding="UTF-8"?>
            <FileZilla3 version="3.67.0" platform="windows">
              <Servers>
                <Server>
                  <Host>192.168.1.10</Host>
                  <Port>22</Port>
                  <Protocol>1</Protocol>
                  <User>deploy</User>
                  <Pass>filezilla-plaintext-secret</Pass>
                  <Logontype>5</Logontype>
                  <Keyfile>C:\keys\id_rsa</Keyfile>
                  <Name>prod server</Name>
                  <POST_LOGIN_COMMANDS>rm -rf /tmp/build</POST_LOGIN_COMMANDS>
                </Server>
                <Server>
                  <Host>ftp.example.com</Host>
                  <Port>21</Port>
                  <Protocol>0</Protocol>
                  <User>ftpuser</User>
                  <Logontype>1</Logontype>
                </Server>
                <Server>
                  <Host>sftp2.example.com</Host>
                  <Port>2222</Port>
                  <Protocol>1</Protocol>
                  <User>root</User>
                  <Logontype>1</Logontype>
                  <Name>site two</Name>
                </Server>
              </Servers>
            </FileZilla3>
            """;

        [Fact]
        public void ParsesKeyFileAndPwSessions_SkipsNonSftp()
        {
            var stat = NewStat(ImportSourceKind.FileZilla);
            var sessions = HostImportParser.ParseFileZillaXml(SampleXml, stat);

            Assert.Equal(2, sessions.Count);
            Assert.Equal(3, stat.Found);
            Assert.Equal(1, stat.SkippedNonSftp);

            var prod = sessions[0];
            Assert.Equal("prod server", prod.Name);
            Assert.Equal("192.168.1.10", prod.Host);
            Assert.Equal(AuthMode.KeyFile, prod.AuthMode);
            Assert.Equal(@"C:\keys\id_rsa", prod.KeyFilePath);

            var second = sessions[1];
            Assert.Equal("sftp2.example.com", second.Host);
            Assert.Equal(2222, second.Port);
            Assert.Equal(AuthMode.Password, second.AuthMode); // Logontype=1
            Assert.Equal(string.Empty, second.KeyFilePath);
        }

        [Fact]
        public void RedLines_NeverReadsPassOrPostLoginCommands()
        {
            var stat = NewStat(ImportSourceKind.FileZilla);
            var sessions = HostImportParser.ParseFileZillaXml(SampleXml, stat);
            AssertNoSecret(sessions, "filezilla-plaintext-secret", "rm -rf");
        }

        [Fact]
        public void DtdEntity_Prohibited()
        {
            const string evil =
                """
                <?xml version="1.0"?>
                <!DOCTYPE FileZilla3 [<!ENTITY xxe SYSTEM "file:///C:/Windows/win.ini">]>
                <FileZilla3><Servers><Server><Host>&xxe;</Host></Server></Servers></FileZilla3>
                """;
            var stat = NewStat(ImportSourceKind.FileZilla);
            Assert.Throws<XmlException>(() => HostImportParser.ParseFileZillaXml(evil, stat));
        }
    }

    // =========================================================================
    // PuTTY 注册表键值映射（internal 直测，免注册表依赖）
    // =========================================================================

    public class PuTTYTests
    {
        private static Dictionary<string, object> Map(params (string K, object V)[] pairs)
        {
            var d = new Dictionary<string, object>(StringComparer.OrdinalIgnoreCase);
            foreach (var (k, v) in pairs) d[k] = v;
            return d;
        }

        [Fact]
        public void SshSession_UserHostSplit_DwordPort()
        {
            var stat = NewStat(ImportSourceKind.PuTTY);
            var s = HostImportParser.FromPuTTYSessionMap(Map(
                ("HostName", "ops@10.0.0.8"),
                ("PortNumber", 2222),
                ("Protocol", "ssh"),
                ("PublicKeyFile", @"C:\keys\putty.ppk"),
                ("RemoteCommand", "tail -f /var/log/syslog")
            ), "putty site", stat);

            Assert.NotNull(s);
            Assert.Equal("putty site", s!.Name);
            Assert.Equal("10.0.0.8", s.Host);
            Assert.Equal("ops", s.Username);
            Assert.Equal(2222, s.Port);
            Assert.Equal(AuthMode.KeyFile, s.AuthMode);
            Assert.Equal(@"C:\keys\putty.ppk", s.KeyFilePath);
        }

        [Fact]
        public void NonSshProtocol_Skipped()
        {
            var stat = NewStat(ImportSourceKind.PuTTY);
            var s = HostImportParser.FromPuTTYSessionMap(Map(
                ("HostName", "serial.example.com"), ("Protocol", "telnet")), "x", stat);
            Assert.Null(s);
            Assert.Equal(1, stat.SkippedNonSftp);
        }

        [Fact]
        public void Defaults_NoProtocolKeyAndNoPort()
        {
            var stat = NewStat(ImportSourceKind.PuTTY);
            var s = HostImportParser.FromPuTTYSessionMap(Map(("HostName", "bare.example.com")), "bare", stat);
            Assert.NotNull(s);
            Assert.Equal(22, s!.Port);
            Assert.Equal(AuthMode.Password, s.AuthMode);
            Assert.Equal(string.Empty, s.Username); // 无 @ → 不拆分
        }

        [Fact]
        public void RedLine_RemoteCommandNeverImported()
        {
            var stat = NewStat(ImportSourceKind.PuTTY);
            var s = HostImportParser.FromPuTTYSessionMap(Map(
                ("HostName", "h"), ("Protocol", "ssh"), ("RemoteCommand", "secret-cmd")), "h", stat);
            Assert.NotNull(s);
            AssertNoSecret(new List<ImportedSession> { s! }, "secret-cmd");
        }

        // N2：独立 UserName 键优先于 HostName 的 user@host 前缀（PuTTY 存独立的 UserName）
        [Fact]
        public void IndependentUserName_TakesPrecedenceOverHostPrefix()
        {
            var stat = NewStat(ImportSourceKind.PuTTY);
            var s = HostImportParser.FromPuTTYSessionMap(Map(
                ("HostName", "ops@10.0.0.8"), ("UserName", "alice"), ("Protocol", "ssh")), "dupe", stat);
            Assert.NotNull(s);
            Assert.Equal("alice", s!.Username);
            Assert.Equal("10.0.0.8", s.Host);
        }

        [Fact]
        public void BlankUserName_FallsBackToHostPrefix()
        {
            var stat = NewStat(ImportSourceKind.PuTTY);
            var s = HostImportParser.FromPuTTYSessionMap(Map(
                ("HostName", "ops@10.0.0.8"), ("UserName", ""), ("Protocol", "ssh")), "x", stat);
            Assert.NotNull(s);
            Assert.Equal("ops", s!.Username); // 独立键为空 → 回退拆分
        }
    }

    // =========================================================================
    // OpenSSH ~/.ssh/config
    // =========================================================================

    public class OpenSshTests
    {
        private static readonly string Home =
            Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);

        private const string SampleConfig =
            """
            # 全局注释
            Host *
                ServerAliveInterval 60

            Host web1
                HostName 192.168.1.20
                User deploy
                Port 2222
                IdentityFile ~/.ssh/id_ed25519
                ProxyCommand nc %h %p

            Host "db server"
                HostName 10.0.0.9
                IdentityFile "%USERPROFILE%\keys\db_key"

            Match host *.corp
                User corpuser

            Host alias-only
            """;

        [Fact]
        public void ParsesConcreteHosts_SkipsWildcardMatchAndAlias()
        {
            var stat = NewStat(ImportSourceKind.OpenSsh);
            var sessions = HostImportParser.ParseOpenSshConfig(SampleConfig, stat);

            Assert.Equal(3, sessions.Count);
            Assert.Equal(3, stat.Found);

            var web1 = sessions[0];
            Assert.Equal("web1", web1.Name);
            Assert.Equal("192.168.1.20", web1.Host);
            Assert.Equal(2222, web1.Port);
            Assert.Equal("deploy", web1.Username);
            Assert.Equal(AuthMode.KeyFile, web1.AuthMode);
            Assert.Equal(Path.Combine(Home, ".ssh", "id_ed25519"), web1.KeyFilePath);

            var db = sessions[1];
            Assert.Equal("db server", db.Name);
            Assert.Equal(Environment.ExpandEnvironmentVariables(@"%USERPROFILE%\keys\db_key"), db.KeyFilePath);

            var alias = sessions[2];
            Assert.Equal("alias-only", alias.Host);          // 无 HostName → pattern 作主机
            Assert.Equal(22, alias.Port);
            Assert.Equal(AuthMode.Password, alias.AuthMode);
        }

        [Fact]
        public void RedLines_ProxyCommandNeverImported()
        {
            var stat = NewStat(ImportSourceKind.OpenSsh);
            var sessions = HostImportParser.ParseOpenSshConfig(SampleConfig, stat);
            AssertNoSecret(sessions, "nc %h");
        }

        [Fact]
        public void IncludeAndUnknownKeys_Ignored()
        {
            const string cfg =
                """
                Include ~/.ssh/config.d/*.conf
                Host h1
                    HostName 10.0.0.1
                    AddKeysToAgent yes
                """;
            var stat = NewStat(ImportSourceKind.OpenSsh);
            var sessions = HostImportParser.ParseOpenSshConfig(cfg, stat);
            Assert.Single(sessions);
            Assert.Equal("10.0.0.1", sessions[0].Host);
        }
    }

    // =========================================================================
    // 自有存档 hosts.json
    // =========================================================================

    public class OwnStoreTests
    {
        private const string SampleJson =
            """
            {
              "hosts": [
                {
                  "Name": "迁移A",
                  "Host": "10.1.1.1",
                  "Port": 2200,
                  "AuthMode": "KeyFile",
                  "KeyFilePath": "C:\\keys\\a",
                  "Username": "ops",
                  "Password": "enc:QUJDREVGRw=="
                },
                {
                  "name": "camel",
                  "host": "10.1.1.2",
                  "port": 22,
                  "username": "u2",
                  "Password": "plaintext-must-not-migrate"
                }
              ]
            }
            """;

        [Fact]
        public void ParsesBothCases_CipherKept_PlainDropped()
        {
            var stat = NewStat(ImportSourceKind.OwnStore);
            var sessions = HostImportParser.ParseOwnStoreJson(SampleJson, stat);

            Assert.Equal(2, sessions.Count);
            Assert.Equal("enc:QUJDREVGRw==", sessions[0].OwnStorePassword);
            Assert.Equal(AuthMode.KeyFile, sessions[0].AuthMode);
            Assert.Equal("10.1.1.2", sessions[1].Host);      // camelCase 兼容
            Assert.Equal(string.Empty, sessions[1].OwnStorePassword); // 明文绝不迁移
        }

        [Fact]
        public void BrokenJson_ThrowsWithCode()
        {
            var stat = NewStat(ImportSourceKind.OwnStore);
            var ex = Assert.Throws<ImportScanException>(
                () => HostImportParser.ParseOwnStoreJson("{ hosts: [", stat));
            Assert.Equal("parseFailed", ex.Code);
        }

        [Fact]
        public void SessionCap_TruncatesAt5000()
        {
            var sb = new StringBuilder("""{"hosts":[""");
            for (var i = 0; i < 5001; i++)
            {
                if (i > 0) sb.Append(',');
                sb.Append($$"""{"Host":"10.0.0.{{i}}","Username":"u"}""");
            }
            sb.Append("]}");

            var stat = NewStat(ImportSourceKind.OwnStore);
            var sessions = HostImportParser.ParseOwnStoreJson(sb.ToString(), stat);
            Assert.Equal(HostImportParser.MaxSessions, sessions.Count);
            Assert.Equal(1, stat.Truncated);
        }
    }

    // =========================================================================
    // ToHostProfile：fail-safe 解密
    // =========================================================================

    public class ToHostProfileTests
    {
        [Fact]
        public void DecryptedPlain_Assigned()
        {
            var s = new ImportedSession
            {
                Kind = ImportSourceKind.OwnStore, Host = "h", Port = 22,
                Username = "u", OwnStorePassword = "enc:CIPHER",
            };
            var p = HostImportParser.ToHostProfile(s, _ => "decrypted-plain");
            Assert.Equal("decrypted-plain", p.Password);
        }

        [Fact]
        public void DecryptionFailure_FailSafeClearsPassword()
        {
            // Unprotect 失败语义：返回原密文（仍带 enc: 前缀）→ 必须清空
            var s = new ImportedSession
            {
                Kind = ImportSourceKind.OwnStore, Host = "h", Port = 22,
                Username = "u", OwnStorePassword = "enc:CIPHER",
            };
            var p = HostImportParser.ToHostProfile(s, _ => "enc:CIPHER");
            Assert.Equal(string.Empty, p.Password);
        }

        [Fact]
        public void NonOwnStore_NeverCarriesPassword()
        {
            var s = new ImportedSession { Kind = ImportSourceKind.FileZilla, Host = "h" };
            var p = HostImportParser.ToHostProfile(s, _ => "should-not-be-called");
            Assert.Equal(string.Empty, p.Password);
        }

        [Fact]
        public void EmptyName_FallsBackToHost()
        {
            var s = new ImportedSession { Host = "1.2.3.4", Port = 22 };
            Assert.Equal("1.2.3.4", HostImportParser.ToHostProfile(s, _ => null).Name);
        }
    }

    // =========================================================================
    // Validate：UNC 拒收 / 密钥不存在告警
    // =========================================================================

    public class ValidateTests
    {
        [Fact]
        public void UncPath_Rejected_FallsBackToPassword()
        {
            var s = new ImportedSession
            {
                AuthMode = AuthMode.KeyFile, KeyFilePath = @"\\nas\share\id_rsa",
            };
            HostImportParser.Validate(s, _ => true);
            Assert.Equal(string.Empty, s.KeyFilePath);
            Assert.Equal(AuthMode.Password, s.AuthMode);
            Assert.Equal("uncPath", s.Warning);
        }

        [Fact]
        public void ForwardSlashUnc_AlsoRejected()
        {
            var s = new ImportedSession
            {
                AuthMode = AuthMode.KeyFile, KeyFilePath = "//nas/share/id_rsa",
            };
            HostImportParser.Validate(s, _ => true);
            Assert.Equal("uncPath", s.Warning);
        }

        [Fact]
        public void MissingKeyFile_KeepsPathWarns()
        {
            var s = new ImportedSession
            {
                AuthMode = AuthMode.KeyFile, KeyFilePath = @"C:\nope\id_rsa",
            };
            HostImportParser.Validate(s, _ => false);
            Assert.Equal(@"C:\nope\id_rsa", s.KeyFilePath); // 保留路径供用户修正
            Assert.Equal(AuthMode.KeyFile, s.AuthMode);
            Assert.Equal("keyNotFound", s.Warning);
        }

        [Fact]
        public void ExistingKeyFile_NoWarning()
        {
            var s = new ImportedSession
            {
                AuthMode = AuthMode.KeyFile, KeyFilePath = @"C:\keys\id_rsa",
            };
            HostImportParser.Validate(s, _ => true);
            Assert.Equal(string.Empty, s.Warning);
        }

        [Fact]
        public void PasswordMode_SkipsValidation()
        {
            var s = new ImportedSession { AuthMode = AuthMode.Password, KeyFilePath = @"\\x\y" };
            HostImportParser.Validate(s, _ => false);
            Assert.Equal(@"\\x\y", s.KeyFilePath); // 未被触碰
            Assert.Equal(string.Empty, s.Warning);
        }
    }

    // =========================================================================
    // DedupKey
    // =========================================================================

    public class DedupKeyTests
    {
        [Fact]
        public void Format_HostPortUsername()
        {
            var s = new ImportedSession { Host = " example.com ", Port = 22, Username = " root " };
            Assert.Equal("example.com:22:root", HostImportParser.DedupKey(s));
        }

        [Fact]
        public void CaseSensitive_ConsistentWithStoreLookup()
        {
            Assert.NotEqual(
                HostImportParser.DedupKey("Example.com", 22, "root"),
                HostImportParser.DedupKey("example.com", 22, "root"));
        }
    }

    // =========================================================================
    // ScanSources：来源白名单（N1——未勾选的来源一个字节都不碰）
    // =========================================================================

    public class ScanSourcesTests
    {
        private const string IniContent =
            "[Sessions\\prod%20server]\n" +
            "HostName=10.9.9.9\n" +
            "PortNumber=22\n" +
            "UserName=root\n" +
            "Protocol=ssh\n";

        private const string OwnStoreContent =
            """{"hosts":[{"Name":"own","Host":"10.1.1.1","Port":22,"Username":"u"}]}""";

        private static string WriteTemp(string fileName, string content)
        {
            var path = Path.Combine(Path.GetTempPath(), fileName);
            File.WriteAllText(path, content);
            return path;
        }

        [Fact]
        public void UncheckedSources_AreNotReadAtAll()
        {
            var iniPath = WriteTemp($"ifreesftp-test-{Guid.NewGuid():N}.ini", IniContent);
            var storePath = WriteTemp($"ifreesftp-test-{Guid.NewGuid():N}.json", OwnStoreContent);
            try
            {
                var result = HostImportParser.ScanSources(new ImportScanOptions
                {
                    WinScpIniPath = iniPath,
                    OwnStorePath = storePath,
                    IncludeRegistry = false,
                    Sources = new HashSet<ImportSourceKind> { ImportSourceKind.OwnStore },
                });

                // 未勾选的 WinScpIni（文件真实存在）：会话、统计、缺失来源三处均不出现
                // （修复前 ScanFileSource 无条件执行，Stats 必含 WinScpIni → 本断言必红）
                Assert.DoesNotContain(result.Sessions, s => s.Kind == ImportSourceKind.WinScpIni);
                Assert.DoesNotContain(result.Stats, t => t.Kind == ImportSourceKind.WinScpIni);
                Assert.DoesNotContain(ImportSourceKind.WinScpIni.ToString(), result.MissingSources);

                // 已勾选的 OwnStore：正常解析
                var own = Assert.Single(result.Sessions);
                Assert.Equal(ImportSourceKind.OwnStore, own.Kind);
                Assert.Equal("10.1.1.1", own.Host);
            }
            finally
            {
                File.Delete(iniPath);
                File.Delete(storePath);
            }
        }

        [Fact]
        public void NoSourceSelected_ProducesEmptyResult()
        {
            // 一个来源都不勾选：不做任何探测，结果全空（而非 MissingSources 一串）
            var result = HostImportParser.ScanSources(new ImportScanOptions
            {
                IncludeRegistry = false,
                Sources = new HashSet<ImportSourceKind>(),
            });

            Assert.Empty(result.Sessions);
            Assert.Empty(result.Stats);
            Assert.Empty(result.MissingSources);
        }
    }
}
