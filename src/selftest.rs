//! Built-in self test: adversarial format cases the parser must survive.
//!
//! These are not unit tests of convenience — every case below is drawn from a
//! real deviation measured in the 507-record library, so a regression here is
//! a regression users would feel.

use crate::parse::parse;
use crate::rec::Lib;
use crate::schema;

struct Case {
    name: &'static str,
    text: &'static str,
    /// Field that must be present after parsing.
    want_key: &'static str,
    /// Substring that must appear in that field's value ("" = any non-empty).
    want_in: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "规范形态",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-1\nKIND=DECISION\nMATURITY=VERIFIED\nAUTHORITY=USER_EXPLICIT\nTITLE=标题\nSUMMARY=摘要\nREAD_WHEN=需要时\nNOT_FOR=不该用时\nKEYS=甲,乙\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "KEYS",
        want_in: "甲",
    },
    Case {
        name: "一行多字段（本库常态）",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-2;PROJECT=p;TASK=t;SESSION=s;SEQUENCE=1\nKIND=DECISION\nTITLE=标题;SUMMARY=摘要;READ_WHEN=需要时;NOT_FOR=不该用时\nKEYS=甲,乙\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "READ_WHEN",
        want_in: "需要时",
    },
    Case {
        name: "值内含半角分号但不构成字段",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-3\nKIND=DECISION\nTITLE=标题\nSUMMARY=前半段;后半段不是字段\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "SUMMARY",
        want_in: "后半段不是字段",
    },
    Case {
        name: "值内含全角分号（全库 2836 次，均非分隔符）",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-4\nKIND=DECISION\nTITLE=标题\nSUMMARY=前半段；ALL_OFF=这不是字段\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "SUMMARY",
        want_in: "ALL_OFF=这不是字段",
    },
    Case {
        name: "缺头部闭合标签（2 例真实）",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-5\nKIND=DECISION\nTITLE=标题\nKEYS=甲\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "KEYS",
        want_in: "甲",
    },
    Case {
        name: "双头部闭合标签（12 例真实）",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-6\nKIND=DECISION\nTITLE=标题\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n[/CANDIDATE_MEMORY]\n",
        want_key: "TITLE",
        want_in: "标题",
    },
    Case {
        name: "正文含标签字面量（3 例真实）",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-7\nKIND=DECISION\nTITLE=标题\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n这里讨论写入时未落 [MEMORY_BODY] 段的情况，正文必须完整。\n[/MEMORY_BODY]\n",
        want_key: "KEYS",
        want_in: "甲",
    },
    Case {
        name: "中文键 注意=",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-8\nKIND=DECISION\nTITLE=标题\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n[PROMOTION]\nPROMOTED_TO=C-1\n注意=正文与头部字段未改写\n",
        want_key: "KEYS",
        want_in: "甲",
    },
    Case {
        name: "键名别名 READ-WHEN",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-9\nKIND=DECISION\nTITLE=标题\nREAD-WHEN=需要时读\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "READ_WHEN",
        want_in: "需要时读",
    },
    Case {
        name: "BOM + CRLF",
        text: "\u{feff}[CANDIDATE_MEMORY]\r\nFORMAT=CANDIDATE_MEMORY_V2\r\nID=N-10\r\nKIND=DECISION\r\nTITLE=标题\r\nKEYS=甲\r\n[/CANDIDATE_MEMORY]\r\n[MEMORY_BODY]\r\n正文内容足够长可以接受\r\n[/MEMORY_BODY]\r\n",
        want_key: "TITLE",
        want_in: "标题",
    },
    Case {
        name: "标签缺方括号",
        text: "CANDIDATE_MEMORY\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-11\nKIND=DECISION\nTITLE=标题\nKEYS=甲\nEND_CANDIDATE_MEMORY\nMEMORY_BODY\n正文内容足够长可以接受\nEND_MEMORY_BODY\n",
        want_key: "TITLE",
        want_in: "标题",
    },
    Case {
        name: "字段顺序打乱（全库 29 种键序）",
        text: "[CANDIDATE_MEMORY]\nKEYS=甲\nTITLE=标题\nID=N-12\nMATURITY=VERIFIED\nKIND=DECISION\nFORMAT=CANDIDATE_MEMORY_V2\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "KEYS",
        want_in: "甲",
    },
    Case {
        name: "末尾无换行（5 例真实）",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-13\nKIND=DECISION\nTITLE=标题\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]",
        want_key: "TITLE",
        want_in: "标题",
    },
    Case {
        name: "值跨多行（2 文件 34 行真实）",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-14\nKIND=DECISION\nTITLE=标题\nSUMMARY=第一行\n第二行继续\n第三行继续\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "SUMMARY",
        want_in: "第三行继续",
    },
    Case {
        name: "重复字段取首个",
        text: "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-15\nKIND=DECISION\nTITLE=第一次\nTITLE=第二次\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        want_key: "TITLE",
        want_in: "第一次",
    },
];

/// Regex engine cases: each is a (pattern, text, should_match) triple.
/// `(pattern, text, case_insensitive, expected)`.
///
/// Every entry is a case where folding has to cross from the literal path into
/// the class path, or where negation could be applied to the wrong thing.
const RX_CI_CASES: &[(&str, &str, bool, bool)] = &[
    // positive class folds
    ("[z]ebra", "Zebra", true, true),
    ("[Z]ebra", "zebra", true, true),
    ("[a-z]ebra", "ZEBRA", true, true),
    // ... and must stop folding when case sensitivity is asked for
    ("[z]ebra", "Zebra", false, false),
    ("[a-z]ebra", "ZEBRA", false, false),
    // negation excludes any variant of the excluded set
    ("[^z]ebra", "Zebra", true, false),
    ("[^Z]ebra", "zebra", true, false),
    ("[^abc]ebra", "Zebra", true, true),
    ("[^YZ]ebra", "Zebra", true, false),
    ("[^Z]ebra", "zebra", false, true),
    // shorthand classes keep working under folding
    (r"\w+", "ABC", true, true),
    (r"\d+", "123", true, true),
];

const RX_CASES: &[(&str, &str, bool)] = &[
    ("GATE_", "MEMORY_ROUTE_GATE_BLOCK 生效", true),
    ("GATE_", "MEMORY_ROUTE_GATE 后接空格", false),
    ("^AB2", "AB2-C-001", true),
    ("^AB2", "C-AB2-001", false),
    ("[A-Z]{3,}_GATE", "ALL_PATH_MEMORY_GATE", true),
    ("[A-Z]{3,}_GATE", "X_GATE", false),
    ("保险箱.*密钥", "保险箱的内嵌密钥", true),
    ("(记忆|回忆)开关", "记忆开关向量", true),
    ("(记忆|回忆)开关", "回忆开关设置", true),
    ("a{2,3}", "baac", true),
    ("a{2,3}", "bac", false),
    ("SEP\\b", "SEP 之后", true),
    ("SEP\\b", "SEPX 之后", false),
    ("\\d+", "版本 v367 定稿", true),
    ("x+$", "末尾xxx", true),
    ("x+$", "xxx末尾", false),
    ("[^a-z]+", "ABC", true),
    ("a|b|c", "zzc", true),
    ("^$", "", true),
    ("保险箱", "凭据保险箱", true),
    // 交替与分组优先级
    ("^(a|b)c$", "ac", true),
    ("^(a|b)c$", "bc", true),
    ("^(a|b)c$", "abc", false),
    // 量词边界
    ("^a{3}$", "aaa", true),
    ("^a{3}$", "aa", false),
    ("^a{2,}$", "aaaaa", true),
    ("^a{2,4}$", "aaaaa", false),
    // 字符类
    ("^[0-9]+$", "20261023", true),
    ("^[0-9]+$", "2026a023", false),
    ("^[^0-9]+$", "abc", true),
    ("^[^0-9]+$", "ab3c", false),
    // 空模式与全匹配
    ("", "任意", true),
    (".*", "任意内容", true),
    ("^.*$", "单行内容", true),
    // 中文锚点
    ("^记忆", "记忆开关", true),
    ("^记忆", "旧记忆开关", false),
    ("开关$", "记忆开关", true),
    // 字节安全：不能把中文切坏
    ("记.开", "记忆开关", true),
    ("[\\x{4e00}-\\x{9fff}]+", "纯中文", false),
];

/// Fuzzy matching: one edit must be tolerated, two must not at k=1.
const FUZZY_CASES: &[(&str, &str, usize, bool)] = &[
    ("记忆开关", "记忆开关", 1, true),
    ("记忆开关", "记忆关开关", 1, true),
    ("记忆开关", "记忆", 1, false),
    ("abc", "abd", 1, true),
    ("abc", "xyz", 1, false),
];

pub fn run(verbose: bool, only: Option<usize>) -> (usize, usize, usize, Vec<String>) {
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut skip = 0usize;
    let mut msgs = Vec::new();
    // `--only N` 只跑第 N 条（1 起）。调试单个用例时不必等全部跑完——
    // 完整自检有几千个子检查，改一处要等一分多钟。
    let mut idx = 0usize;
    for c in CASES {
        idx += 1;
        if let Some(n) = only {
            if idx != n {
                continue;
            }
        }
        let p = parse(c.text, Some(Lib::Candidate));
        let got = p.get(c.want_key);
        let ok = if p.has_error() {
            false
        } else if got.trim().is_empty() {
            false
        } else if c.want_in.is_empty() {
            true
        } else {
            got.contains(c.want_in)
        };
        if ok {
            pass += 1;
            if verbose {
                msgs.push(format!("PASS  {}", c.name));
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  {}  ::  期望 {} 含 {:?}，实得 {:?}{}",
                c.name,
                c.want_key,
                c.want_in,
                crate::util::truncate_chars(&got, 60),
                if p.has_error() { "（解析报错）" } else { "" }
            ));
        }
    }

    // `--only N` 到此为止：它只跑第 N 条解析用例，不跑后面那一百多个检查块。
    //
    // 不在这里返回会变成"看起来限定了、其实全跑了"——参数说了只跑一条，
    // 结果该等的一分钟还是等一分钟，那这个参数就是在骗人。要么做到，要么
    // 别广告它。
    if only.is_some() {
        return (pass, fail, skip, msgs);
    }

    // Schema layer: a record missing an identifying field must be an ERROR,
    // while a record merely missing READ_WHEN must stay usable (WARN).
    let good = parse(
        "[CANDIDATE_MEMORY]\nID=N-20\nTITLE=标题\nKIND=DECISION\nMATURITY=VERIFIED\nAUTHORITY=USER_EXPLICIT\nSUMMARY=摘要\nKEYS=甲\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        Some(Lib::Candidate),
    );
    let rep = schema::validate(&good, Lib::Candidate, None);
    if !rep.has_error() {
        pass += 1;
        if verbose {
            msgs.push("PASS  缺 READ_WHEN 仍可用（WARN 而非 ERROR）".into());
        }
    } else {
        fail += 1;
        msgs.push("FAIL  缺 READ_WHEN 被误判为 ERROR".into());
    }

    let bad = parse(
        "[CANDIDATE_MEMORY]\nKIND=DECISION\nSUMMARY=摘要\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n",
        Some(Lib::Candidate),
    );
    let rep2 = schema::validate(&bad, Lib::Candidate, None);
    if rep2.has_error() {
        pass += 1;
        if verbose {
            msgs.push("PASS  缺 ID/TITLE 被正确判为 ERROR".into());
        }
    } else {
        fail += 1;
        msgs.push("FAIL  缺 ID/TITLE 未被判为 ERROR".into());
    }

    // Deviation accounting: a clean record has zero deviations.
    if good.devs.is_empty() {
        pass += 1;
        if verbose {
            msgs.push("PASS  规范记录零偏差".into());
        }
    } else {
        fail += 1;
        msgs.push(format!("FAIL  规范记录仍有 {} 处偏差", good.devs.len()));
    }

    if parse("我们聊聊别的吧\n完全不是记录", None).has_error() {
        pass += 1;
        if verbose {
            msgs.push("PASS  非记录文本被拒".into());
        }
    } else {
        fail += 1;
        msgs.push("FAIL  非记录文本未被拒".into());
    }

    // ---- regex engine
    for (pat, text, want) in RX_CASES {
        let got = match crate::rx::Regex::new(pat, false) {
            Ok(re) => re.is_match(text),
            Err(_) => {
                fail += 1;
                msgs.push(format!("FAIL  正则无法编译: {pat}"));
                continue;
            }
        };
        if got == *want {
            pass += 1;
            if verbose {
                msgs.push(format!("PASS  正则 {pat} 对 {text:?}"));
            }
        } else {
            fail += 1;
            msgs.push(format!("FAIL  正则 {pat} 对 {text:?} 期望 {want} 实得 {got}"));
        }
    }

    // ---- case folding must reach character classes, and negation must apply
    //      to the whole set of variants
    //
    // A bare literal already folded (`zebra` matched `Zebra`) while `[z]ebra`
    // did not, because the flag was recorded on literals and never consulted for
    // classes. Half the engine honoured `--case` and half ignored it.
    //
    // The negation half is the one that bites: `[^abc]` admitting `A` looks like
    // "case insensitive is on, fine", and only shows up as a class that refuses
    // to exclude anything.
    for (pat, text, ci, want) in RX_CI_CASES {
        let got = match crate::rx::Regex::new(pat, *ci) {
            Ok(re) => re.is_match(text),
            Err(_) => {
                fail += 1;
                msgs.push(format!("FAIL  正则无法编译: {pat} (ci={ci})"));
                continue;
            }
        };
        if got == *want {
            pass += 1;
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  正则 {pat} 对 {text:?} (ci={ci}) 期望 {want} 实得 {got}"
            ));
        }
    }

    // ---- fuzzy distance
    for (pat, text, k, want) in FUZZY_CASES {
        let a: Vec<char> = pat.chars().collect();
        let t: Vec<char> = text.chars().collect();
        let got = crate::search::fuzzy_best_pub(&t, &a, *k).is_some();
        if got == *want {
            pass += 1;
            if verbose {
                msgs.push(format!("PASS  模糊 {pat} 对 {text:?} (k={k})"));
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  模糊 {pat} 对 {text:?} 期望 {want} 实得 {got}"
            ));
        }
    }

    // ---- lint and audit must agree
    // `lint` and `audit` are two front-ends over the same records. If they
    // report different totals the user cannot tell which to trust. The check
    // must compare the two *front-ends*, not the validator against itself --
    // an earlier version did the latter and passed even when lint was broken.
    {
        let root = crate::resolve_root(&crate::util::Args::default());
        if root.exists() {
            let mut total = 0usize;
            let mut audit_clean = 0usize;
            let mut lint_bad = 0usize;
            for lib in [crate::rec::Lib::Canonical, crate::rec::Lib::Candidate] {
                for rec in crate::rec::load_lib(&root, lib) {
                    total += 1;
                    // what `audit` counts as clean
                    let rep = crate::schema::validate(
                        &rec.as_parsed(),
                        rec.lib,
                        Some(&rec.file_name()),
                    );
                    if rep.clean() {
                        audit_clean += 1;
                    }
                    // what `lint` prints per record
                    if !rec.lint().is_empty() {
                        lint_bad += 1;
                    }
                }
            }
            if total == 0 {
                fail += 1;
                msgs.push("FAIL  lint/audit 一致性：库为空，无法验证".to_string());
            } else if audit_clean + lint_bad != total {
                fail += 1;
                msgs.push(format!(
                    "FAIL  lint/audit 不一致：audit 规范 {audit_clean} + lint 有问题 {lint_bad} = {}，但总数 {total}",
                    audit_clean + lint_bad
                ));
            } else {
                pass += 1;
                if verbose {
                    msgs.push(format!(
                        "PASS  lint/audit 一致：{total} 条中 {audit_clean} 规范 / {lint_bad} 有问题"
                    ));
                }
            }
        }
    }

    // ---- GitHub release JSON parsing must tolerate formatting
    // A real defect: matching `"name":"x"` literally failed against GitHub's
    // pretty-printed `"name": "x"`, and the failure looked like "asset not
    // found". Parse three shapes so the assumption cannot come back.
    {
        // (json, wanted name, must it be found?, expected id when found)
        let cases = [
            // pretty-printed, exactly what GitHub returns today
            (r#"{ "url": "u", "id": 111, "name": "abook-aarch64-linux-android" }"#,
             "abook-aarch64-linux-android", true, Some(111u64)),
            // minified
            (r#"{"id":222,"name":"abook-x86_64-unknown-linux-musl"}"#,
             "abook-x86_64-unknown-linux-musl", true, Some(222)),
            // an unrelated asset must not match
            (r#"{ "id": 333, "name": "abook-aarch64-apple-darwin" }"#,
             "abook-i686-linux-android", false, None),
        ];
        let mut good = 0;
        for (json, want, should, expect_id) in cases.iter() {
            let got = crate::asset_id_for(json, want);
            let ok = if *should { got == *expect_id } else { got.is_none() };
            if ok {
                good += 1;
            }
        }
        if good == 3 {
            pass += 1;
            if verbose {
                msgs.push("PASS  release JSON 解析：紧凑/美化/无关三种形态都正确".to_string());
            }
        } else {
            fail += 1;
            msgs.push(format!("FAIL  release JSON 解析：3 例中只有 {good} 例正确"));
        }
    }

    // ---- downloaded bytes must be recognised as executables
    {
        let elfin = [0x7fu8, b'E', b'L', b'F', 2, 1, 1, 0];
        let mz = [b'M' as u8, b'Z' as u8, 0, 0];
        let html = b"<!DOCTYPE html><html>Not Found</html>";
        let ok = crate::looks_like_binary(&elfin, "linux")
            && crate::looks_like_binary(&mz, "windows")
            && !crate::looks_like_binary(html, "linux")
            && !crate::looks_like_binary(b"Not Found", "windows");
        if ok {
            pass += 1;
            if verbose {
                msgs.push("PASS  下载产物校验：ELF/MZ 通过，HTML 与短文本拒绝".to_string());
            }
        } else {
            fail += 1;
            msgs.push("FAIL  下载产物校验未按魔数判断".to_string());
        }
    }

    // ---- "installed" must not be satisfied by a throw-away location
    {
        let cases = [
            ("/home/u/.local/bin/abook", "stable"),
            ("/usr/bin/abook", "stable"),
            ("/data/x/.dsh/workspaces/incoming/abook/target/release/abook", "transient"),
            ("/src/abook/target/debug/abook", "transient"),
        ];
        let mut good = 0;
        for (path, want) in cases.iter() {
            if crate::install_location_kind(path) == *want {
                good += 1;
            }
        }
        if good == cases.len() {
            pass += 1;
            if verbose {
                msgs.push("PASS  安装位置判定：构建目录/工作区不算已安装".to_string());
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  安装位置判定：{} 例中只有 {good} 例正确",
                cases.len()
            ));
        }
    }

    // ---- a mistyped parameter must be rejected, not ignored
    // Returning the whole library for `--limt 5` looked like success, which is
    // worse than an error: the caller believes it paged.
    {
        let mut good = 0;
        let total = 5;
        // a real name is accepted
        if crate::util::Args::known("per-page") {
            good += 1;
        }
        // a typo is not
        if !crate::util::Args::known("limt") {
            good += 1;
        }
        // every known name is listed exactly once (a stale list would reject
        // a legitimate parameter)
        let listed: Vec<&str> = crate::util::Args::all_known().collect();
        let uniq: std::collections::BTreeSet<&str> = listed.iter().copied().collect();
        if listed.len() == uniq.len() {
            good += 1;
        }
        // the alias table actually redirects the common spellings
        let argv: Vec<String> = ["list", "--limit", "3"].iter().map(|s| s.to_string()).collect();
        if let Ok(a) = crate::util::Args::parse(&argv) {
            if a.num("per-page") == Some(3) {
                good += 1;
            }
        }
        let argv2: Vec<String> = ["list", "--size", "7"].iter().map(|s| s.to_string()).collect();
        if let Ok(a) = crate::util::Args::parse(&argv2) {
            if a.num("per-page") == Some(7) {
                good += 1;
            }
        }
        if good == total {
            pass += 1;
            if verbose {
                msgs.push("PASS  参数校验：拼错即报错，--limit/--size 归一到分页".to_string());
            }
        } else {
            fail += 1;
            // Report which sub-check failed, otherwise the message is useless.
            let mut which: Vec<&str> = Vec::new();
            if !crate::util::Args::known("per-page") {
                which.push("per-page 未被识别");
            }
            if crate::util::Args::known("limt") {
                which.push("拼错的名字被当成合法");
            }
            if listed.len() != uniq.len() {
                which.push("已知参数表有重复登记");
            }
            let a1 = crate::util::Args::parse(&argv).ok();
            if a1.as_ref().and_then(|a| a.num("per-page")) != Some(3) {
                which.push("--limit 未归一");
            }
            let a2 = crate::util::Args::parse(&argv2).ok();
            if a2.as_ref().and_then(|a| a.num("per-page")) != Some(7) {
                which.push("--size 未归一");
            }
            msgs.push(format!(
                "FAIL  参数校验：{} 项中只有 {good} 项正确 [{}]",
                total,
                which.join("，")
            ));
        }
    }

    // ---- every command the help lists must actually exist
    // `vault lint`, `vault repair`, `vault import`, `vault rekey` were all
    // documented but not dispatched, so a documented command answered
    // "unknown subcommand". Comparing the help text against the dispatch table
    // catches that class mechanically instead of relying on someone testing it.
    {
        let help = crate::help_text();
        // collect the bare command words from the top-level command listing
        let mut listed: Vec<String> = Vec::new();
        for line in help.lines() {
            let t = line.trim_start();
            if !line.starts_with("  ") || t.starts_with(' ') || t.starts_with('-') {
                continue;
            }
            if let Some(first) = t.split_whitespace().next() {
                if first.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                    && first.len() > 1
                    && !matches!(first, "abook" | "vault" | "真" )
                {
                    listed.push(first.to_string());
                }
            }
        }
        listed.sort();
        listed.dedup();
        let known = crate::known_commands();
        let missing: Vec<&String> = listed.iter().filter(|c| !known.contains(&c.as_str())).collect();
        if missing.is_empty() {
            pass += 1;
            if verbose {
                msgs.push(format!("PASS  帮助与实际命令一致（核对 {} 个）", listed.len()));
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  帮助里写了但实际没有的命令: {}",
                missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
    }

    // ---- every advertised search tier must actually route somewhere different
    //
    // `--deep` was read only inside `semantic_search`, so `search 词 --deep`
    // never reached it: the flag was accepted, cost nothing, and returned exactly
    // what `search 词` returned. The most expensive tier quietly did the least
    // work, which is worse than being slow because nothing signals the miss.
    //
    // Verified through the CLI rather than by calling the router, because the
    // defect was in the routing and an in-process call would skip it.
    if let Some(exe) = self_path() {
        let root = crate::util::resolve_root(&crate::util::Args::parse(&[]).unwrap_or_default());
        if root.join("canonical/records").is_dir() {
            let probe = |extra: &[&str]| -> (usize, String) {
                let mut args = vec![
                    "--root".to_string(),
                    root.to_string_lossy().to_string(),
                    "search".to_string(),
                    "记忆".to_string(),
                    "--max".to_string(),
                    "1".to_string(),
                ];
                for e in extra {
                    args.push((*e).to_string());
                }
                match std::process::Command::new(&exe).args(&args).output() {
                    Ok(o) => {
                        let t = String::from_utf8_lossy(&o.stdout).to_string();
                        // first result line looks like ` 112  path  [field] title`
                        let first = t
                            .lines()
                            .find(|l| {
                                l.trim_start().starts_with(|c: char| c.is_ascii_digit())
                                    && l.contains("records/")
                            })
                            .unwrap_or("")
                            .to_string();
                        (t.len(), first)
                    }
                    Err(_) => (0, String::new()),
                }
            };
            let (_, l1) = probe(&[]);
            let (_, l5) = probe(&["--deep"]);
            let (_, l3) = probe(&["--level", "3"]);
            if l1.is_empty() || l5.is_empty() {
                skip += 1;
                msgs.push("SKIP  真库不足，跳过分档路由核对".into());
            } else {
                // One assertion per tier. Requiring *both* to collapse before
                // failing meant removing just one route still passed: the other
                // tier kept the comparison honest while the broken one went
                // unnoticed. Each tier is independently advertised, so each is
                // independently checked.
                let mut bad: Vec<&str> = Vec::new();
                if l5 == l1 {
                    bad.push("--deep");
                }
                if l3 == l1 {
                    bad.push("--level 3");
                }
                if bad.is_empty() {
                    pass += 1;
                    msgs.push("PASS  分档路由：--deep / --level 3 与 L1 返回不同结果".into());
                } else {
                    fail += 1;
                    msgs.push(format!(
                        "FAIL  分档没生效：{} 返回了和 L1 相同的结果 {l1:?}",
                        bad.join(" 与 ")
                    ));
                }
            }
        } else {
            skip += 1;
            msgs.push("SKIP  无真库，跳过分档路由核对".into());
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过分档路由核对".into());
    }

    // ---- Win32 timezone struct layout
    //
    // The Windows build asks the OS for the local UTC offset, because without it
    // every timestamp written there lands at `+00:00` while the library's own
    // records are `+08:00[Asia/Shanghai]`. Wrong field order does not fail to
    // compile -- it silently reads a different number -- so the layout is pinned
    // here rather than trusted to review. The struct is deliberately compiled on
    // all platforms so this assertion runs where it can be checked.
    {
        let mut bad_l = Vec::new();
        let st = std::mem::size_of::<crate::util::WinSystemTime>();
        if st != 16 {
            bad_l.push(format!("SYSTEMTIME 大小 {st}，应为 16"));
        }
        let tz = std::mem::size_of::<crate::util::WinTimeZoneInfo>();
        if tz != 172 {
            bad_l.push(format!("TIME_ZONE_INFORMATION 大小 {tz}，应为 172"));
        }
        // Total size alone is not enough, and believing it was cost a false
        // negative: shrinking `standard_name` from 32 to 31 u16 removes two
        // bytes that `#[repr(C)]` padding hands straight back, so the struct
        // still measures 172 while `standard_date` has moved down two bytes.
        // The OS would then write a transition date into the middle of the name
        // array and every read after that point is garbage -- with a matching
        // size check reporting "layout correct". Pin the offsets.
        let offs: [(&str, usize, usize); 7] = [
            ("bias", std::mem::offset_of!(crate::util::WinTimeZoneInfo, bias), 0),
            ("standard_name", std::mem::offset_of!(crate::util::WinTimeZoneInfo, standard_name), 4),
            ("standard_date", std::mem::offset_of!(crate::util::WinTimeZoneInfo, standard_date), 68),
            ("standard_bias", std::mem::offset_of!(crate::util::WinTimeZoneInfo, standard_bias), 84),
            ("daylight_name", std::mem::offset_of!(crate::util::WinTimeZoneInfo, daylight_name), 88),
            ("daylight_date", std::mem::offset_of!(crate::util::WinTimeZoneInfo, daylight_date), 152),
            ("daylight_bias", std::mem::offset_of!(crate::util::WinTimeZoneInfo, daylight_bias), 168),
        ];
        for (name, got, want) in offs {
            if got != want {
                bad_l.push(format!("{name} 偏移 {got}，应为 {want}"));
            }
        }
        if bad_l.is_empty() {
            pass += 1;
            msgs.push("PASS  Win32 时区结构体布局正确（172 字节，7 个字段偏移全对）".into());
        } else {
            fail += 1;
            msgs.push(format!("FAIL  Win32 时区结构体布局: {}", bad_l.join("；")));
        }
    }

    // ---- a command that advertises paging must actually page
    //
    // `spec_for` once appended `page`/`per-page` to every command, so about
    // fifty of them accepted the flags, validated them, and then printed the
    // whole list. `stats --per-page 3` and `stats` returned the same bytes and
    // the exit code said success either way, so a caller paging through results
    // had no way to learn the flag did nothing.
    //
    // The global append is gone. What remains unproven is the other direction:
    // a row that lists `page,per-page` in its own params must honour them. So
    // for each such command, run it twice -- once plain, once with
    // `--per-page 2` -- and require the output to differ. Identical output from
    // a command that claims to page is the flag being ignored again.
    //
    // Only commands whose sample invocation needs no record id are exercised.
    // The rest can read a real record from the sample library when one is
    // present; when it is not, they are skipped rather than guessed at.
    if let Some(exe) = self_path() {
        // Build a fixture with enough rows instead of borrowing the real library.
        //
        // The first version probed the real library with `--per-page 2` and
        // reported `staging` and `budget` as broken. `staging` held exactly one
        // file, so a two-row page cuts nothing and the output is legitimately
        // identical -- a false positive created by the fixture, not a defect in
        // the command. A test that only works when the data happens to be big
        // enough reports on the data, not on the code.
        let tmp = std::env::temp_dir().join(format!("abook-pg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let lib = tmp.join("lib");
        let _ = std::fs::create_dir_all(&lib);
        let mut ready = false;
        if std::process::Command::new(&exe)
            .args(["--root", &lib.to_string_lossy(), "init"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            ready = true;
        }
        if ready {
            // Five records, five staging notes, five review notes, five commits:
            // enough that a one-row page must split, for every paged command.
            for i in 0..5 {
                let canon = format!(
                    "[CANONICAL_MEMORY]\nFORMAT=CANONICAL_MEMORY_V2\nID=C{i}\nTITLE=fixture {i}\nSTATUS=ACTIVE\nKEYS=k{i}\nREAD_WHEN=always\n[/CANONICAL_MEMORY]\n[MEMORY_BODY]\nbody {i}\n[/MEMORY_BODY]\n"
                );
                let _ = std::fs::write(lib.join(format!("canonical/records/C{i}.md")), canon);
                let cand = format!(
                    "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N{i}\nTITLE=cand {i}\nSTATUS=ACTIVE\nMATURITY=OBSERVED\nKEYS=k{i}\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\ncand body {i}\n[/MEMORY_BODY]\n"
                );
                let _ = std::fs::write(lib.join(format!("candidate/records/N{i}.md")), cand);
                let _ = std::fs::write(
                    lib.join(format!("staging/note-{i}.md")),
                    format!("待决暂存件 {i}\n"),
                );
                let _ = std::fs::write(
                    lib.join(format!("review/review-{i}.md")),
                    format!("[REVIEW]\nID=R{i}\nSUMMARY=review {i}\n[/REVIEW]\n"),
                );
                let _ = std::fs::write(
                    lib.join(format!("commits/2026-01-0{}T00:00:00+08:00-c{i}.md", i + 1)),
                    format!("[COMMIT]\nFORMAT=COMMIT_V1\nID=K{i}\nAT=2026-01-0{}T00:00:00+08:00\nKIND=set-field\nSUMMARY=commit {i}\n[/COMMIT]\n", i + 1),
                );
            }
            // Three atlas branches, so `branch list` has something to page.
            for name in ["canonical", "candidate"] {
                let mut atlas = String::from("COVERED_THROUGH=2026-01-01T00:00:00+08:00\n");
                for b in 0..3 {
                    atlas.push_str(&format!(
                        "BRANCH=b{b}\nKEYS=k{b}\nCOUNT=2\nMAXSIZE=SMALL\nSUMMARY=s{b}\nREAD_WHEN=always\nSTATUS=ACTIVE\nPATH={name}/records/C{b}.md\n\n"
                    ));
                }
                let _ = std::fs::write(lib.join(format!("{name}/Atlas.md")), atlas);
            }
            // 用真实的 `BRANCH` 表格式。原来这里是 `| 主题 | 位置｜`,而 `routes()`
            // 只认表头含 BRANCH/分支的表格——于是这个测试库一条路由都解析不出,
            // `index` 只打一行标题。分页核对在"零行可翻"的数据上永远通过,它测的
            // 是数据不是代码,直到别处给输出多添两行才暴露出它一直没在真的检查。
            // 用真实的 `BRANCH` 表格式，而且必须包在 `[MEMORY_CONTENT]` 里。
            //
            // 这里原来写的是 `| 主题 | 位置｜` 且没有正文标记，两处都不对：`routes()`
            // 只认表头含 BRANCH/分支的表格，而 `BookFile::load` 只取
            // `[MEMORY_CONTENT]` 与 `[/MEMORY_CONTENT]` 之间的内容。于是这个测试库
            // 从建成起就解析出零条路由，`index` 只打一行标题——分页核对在"没有行可
            // 翻"的数据上永远通过，它测的是数据不是代码。
            let _ = std::fs::write(
                lib.join("Index.md"),
                "[MEMORY_FILE_CONTENTS]\nFORMAT=AGENTSBOOK_V2_EXPERIMENT\nFILE=Index.md\n[/MEMORY_FILE_CONTENTS]\n\
                 [MEMORY_CONTENT]\n\
                 | BRANCH | 正式 / 候选 | READ_WHEN | 地图 |\n|---|---|---|---|\n\
                 | `b0` | 2 / 0 | always | `canonical/Atlas.md` |\n\
                 | `b1` | 2 / 0 | always | `canonical/Atlas.md` |\n\
                 | `b2` | 2 / 0 | always | `canonical/Atlas.md` |\n\
                 [/MEMORY_CONTENT]\n",
            );
            let _ = std::fs::write(lib.join("Global.md"), "# Global\n\n测试库\n");
        }

        let mut ignored: Vec<&str> = Vec::new();
        let mut checked = 0usize;
        if ready {
            let probes: &[(&str, &[&str])] = &[
                ("atlas", &[]),
                ("index", &[]),
                ("commands", &[]),
                ("describe", &[]),
                ("list", &[]),
                ("recent", &[]),
                ("log", &[]),
                ("review", &[]),
                ("staging", &[]),
                ("tree", &[]),
                ("branch", &["list"]),
                ("state", &[]),
            ];
            let lr = lib.to_string_lossy().to_string();
            for (cmd, extra) in probes {
                let mut base: Vec<String> = vec!["--root".into(), lr.clone(), (*cmd).into()];
                for e in *extra {
                    base.push((*e).to_string());
                }
                let plain = std::process::Command::new(&exe).args(&base).output();
                let mut one = base.clone();
                one.push("--per-page".into());
                one.push("1".into());
                let paged = std::process::Command::new(&exe).args(&one).output();
                let (po, qo) = match (plain, paged) {
                    (Ok(a), Ok(b)) => (a, b),
                    _ => continue,
                };
                // Refusing the flag is honest: the command says it has nothing
                // to page. Only a command that accepts it *and* ignores it is
                // the defect this case exists to catch.
                if qo.status.code() != Some(0) {
                    continue;
                }
                let a = String::from_utf8_lossy(&po.stdout).to_string();
                let b = String::from_utf8_lossy(&qo.stdout).to_string();
                if a.lines().count() < 3 {
                    continue;
                }
                checked += 1;
                if a == b {
                    ignored.push(cmd);
                }
            }
        }
        if !ready {
            skip += 1;
            msgs.push("SKIP  无法造测试库，跳过分页核对".into());
        } else if checked == 0 {
            skip += 1;
            msgs.push("SKIP  无可验证的分页命令".into());
        } else if ignored.is_empty() {
            pass += 1;
            msgs.push(format!(
                "PASS  分页名副其实：{checked} 个声明分页的命令都真的分页"
            ));
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  声明了分页但 --per-page 不生效: {}",
                ignored.join(", ")
            ));
        }
        let _ = std::fs::remove_dir_all(&tmp);
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过分页核对".into());
    }

    // ---- `--field` must select the field, not silently fall back to the body
    //
    // The documented form is `patch X --field TITLE --old a --new b`. The code
    // only honoured `--field` when `--in field` was also present, so the
    // documented form edited the *body*. Nothing failed visibly: the body almost
    // always contains the pattern too, the replacement succeeds, and the caller
    // sees success while the field they meant to change is untouched.
    //
    // The assertion is on the field, because that is the thing the caller named.
    if let Some(exe) = self_path() {
        let tmp = std::env::temp_dir().join(format!("abook-pf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(tmp.join("canonical/records"));
        let rec = "[CANONICAL_MEMORY]\nFORMAT=CANONICAL_MEMORY_V2\nID=X1\nTITLE=aa-aa\nSTATUS=ACTIVE\nKEYS=aa,bb\n[/CANONICAL_MEMORY]\n[MEMORY_BODY]\nbodyhere\n[/MEMORY_BODY]\n";
        let p = tmp.join("canonical/records/X1.md");
        let run = |args: &[&str]| -> String {
            match std::process::Command::new(&exe).args(args).output() {
                Ok(o) => format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => format!("{e}"),
            }
        };
        let t = tmp.to_string_lossy().to_string();
        // multiple hits, no --all: must refuse and leave the field alone
        let _ = std::fs::write(&p, rec);
        let r1 = run(&["--root", &t, "patch", "X1", "--field", "TITLE", "--old", "aa", "--new", "zz", "--approve"]);
        let f1 = run(&["--root", &t, "get", "X1", "--field", "TITLE"]);
        // --all: replaces every hit
        let r2 = run(&["--root", &t, "patch", "X1", "--field", "TITLE", "--old", "aa", "--new", "zz", "--approve", "--all"]);
        let f2 = run(&["--root", &t, "get", "X1", "--field", "TITLE"]);
        // body still reachable without --field
        let _ = std::fs::write(&p, rec);
        let r3 = run(&["--root", &t, "patch", "X1", "--old", "bodyhere", "--new", "B2", "--approve"]);
        let raw = std::fs::read_to_string(&p).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&tmp);
        let ok = r1.contains("--all")
            && f1.trim() == "aa-aa"
            && r2.contains("已替换 2")
            && f2.trim() == "zz-zz"
            && r3.contains("已替换 1")
            && raw.contains("B2")
            && raw.contains("TITLE=aa-aa");
        if ok {
            pass += 1;
            msgs.push("PASS  patch --field 选中字段，多命中需 --all，正文仍可单独改".into());
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  patch --field 语义不对: 多命中={:?} 字段={:?} all={:?} 结果={:?} 正文={}",
                r1.trim(),
                f1.trim(),
                r2.trim(),
                f2.trim(),
                raw.contains("B2")
            ));
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过 patch --field 检查".into());
    }

    // ---- every field name in the library must be one the parser knows
    //
    // `KNOWN_KEYS` was written early and then stranded: the tolerant parser
    // decides "is this `KEY=` the start of a new field or a continuation of the
    // previous value" by checking membership in it. Leaving it unreferenced meant
    // the one list that governs that decision was never checked against the live
    // library, so a genuine field missing from it would silently be read as
    // continuation prose -- the exact failure that once invented fields named
    // `围栏18` and `符号定义265`.
    //
    // Reading the real field names back is the only way to catch that, and it has
    // to run against the actual library: a fixture can only contain keys someone
    // remembered to put in it.
    if let Some(exe) = self_path() {
        let root = crate::util::resolve_root(&crate::util::Args::parse(&[]).unwrap_or_default());
        if root.join("canonical/records").is_dir() {
            let out = match std::process::Command::new(&exe)
                .args([
                    "--root",
                    &root.to_string_lossy(),
                    "list",
                    "--json",
                    "--limit",
                    "100000",
                ])
                .output()
            {
                Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                Err(_) => String::new(),
            };
            // Collect field names by running the *real* parser over every
            // record, not by hand-splitting lines.
            //
            // The earlier version scanned with `line.split(';')` + `find('=')`
            // + `trim()`, which is wider than `parse::key_end_at` (that one
            // bails on a space, so `TOKEN = re.compile(...)` is not a field).
            // The mismatch mattered twice: it reported twelve names that live in
            // record *bodies*, and `set("FILES")`-style entries as fields -- all
            // false positives that hid the real signal. Asking `Record::parse`
            // makes this check and the parser agree by construction.
            //
            // `.take(400)` is gone too: the library already holds 523 canonical
            // records, so a 400-file sample reported whatever `read_dir`
            // happened to yield first and its list drifted between runs.
            let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let mut files = 0usize;
            let mut parse_failed = 0usize;
            for (lib, sub) in [
                (crate::rec::Lib::Canonical, "canonical/records"),
                (crate::rec::Lib::Candidate, "candidate/records"),
            ] {
                let d = root.join(sub);
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                let mut paths: Vec<std::path::PathBuf> =
                    rd.flatten().map(|e| e.path()).collect();
                paths.sort();
                for p in paths {
                    if p.extension().and_then(|x| x.to_str()) != Some("md") {
                        continue;
                    }
                    let Ok(txt) = std::fs::read_to_string(&p) else { continue };
                    files += 1;
                    match crate::rec::Record::parse_str(&txt, &p, lib) {
                        Ok(r) => {
                            for (k, _) in &r.fields {
                                seen.insert(k.clone());
                            }
                        }
                        Err(_) => parse_failed += 1,
                    }
                }
            }
            let unknown: Vec<&String> = seen
                .iter()
                .filter(|k| !crate::rec::is_known_key(k))
                .collect();
            // A handful of container/structural names are not record fields.
            const STRUCTURAL: &[&str] = &[
                "FORMAT", "FILE", "ENTRY", "CONTENTS", "BRANCH", "KEYS", "COUNT", "MAXSIZE",
                "SUMMARY", "READ_WHEN", "PATH", "REVISION", "STATUS", "TOTAL", "PAGE",
            ];
            let real_unknown: Vec<&&String> = unknown
                .iter()
                .filter(|k| !STRUCTURAL.contains(&k.as_str()))
                .collect();
            let _ = out;
            if files > 0 && real_unknown.is_empty() {
                pass += 1;
                msgs.push(format!(
                    "PASS  真库 {} 条样本的字段名都在已知键表内（{} 个键）",
                    files,
                    seen.len()
                ));
            } else if real_unknown.is_empty() {
                pass += 1;
                msgs.push("PASS  无真库可读，跳过字段名核对".into());
            } else {
                fail += 1;
                let names: Vec<String> = real_unknown.iter().map(|k| (**k).clone()).collect();
                msgs.push(format!(
                    "FAIL  这些字段名不在已知键表内，会被当成上一字段的续行: {}",
                    names.join("、")
                ));
            }
        } else {
            skip += 1;
            msgs.push("SKIP  无真库，跳过字段名核对".into());
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过字段名核对".into());
    }

    // ---- the credential identity index must be readable without the vault
    //
    // The spec designates this section as a per-turn read and forbids values in
    // it precisely so that learning *what exists* needs no decryption. The
    // parser existed and had no caller, so the only way to enumerate credentials
    // was `vault ledger`, which lives inside the encrypted box.
    if let Some(exe) = self_path() {
        let tmp = std::env::temp_dir().join(format!("abook-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(&tmp);
        // Same container shape as the real Global.md. BookFile only reads the
        // body inside [MEMORY_CONTENT]; a fixture without the markers parses as
        // zero rows and would "pass" an emptiness test while proving nothing.
        let _ = std::fs::write(
            tmp.join("Global.md"),
            "[MEMORY_BOOK_V2]\nFORMAT=AGENTSBOOK_V2_EXPERIMENT\nFILE=Global.md\n[/MEMORY_BOOK_V2]\n[MEMORY_CONTENT]\n# Global.md\n\n## Credential Identity Index\n\n标识｜用途｜归属｜允许｜取用｜披露\nmytoken｜构建推送｜自有｜仅限本任务｜按需取单条｜默认不回显\n\n## Map Index\n\n无关内容｜不应出现\n[/MEMORY_CONTENT]\n",
        );
        let run = |args: &[&str]| -> String {
            match std::process::Command::new(&exe).args(args).output() {
                Ok(o) => format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => format!("{e}"),
            }
        };
        let t = tmp.to_string_lossy().to_string();
        let plain = run(&["--root", &t, "identity"]);
        let js = run(&["--root", &t, "identity", "--json"]);
        let _ = std::fs::remove_dir_all(&tmp);
        // Must find the credential, must not bleed the neighbouring section in,
        // and the JSON must parse far enough to carry the fields.
        let ok = plain.contains("mytoken")
            && !plain.contains("无关内容")
            && js.contains("\"id\":\"mytoken\"")
            && js.contains("\"when\"");
        if ok {
            pass += 1;
            msgs.push("PASS  identity 只读身份索引，不含值也不串入其它段".into());
        } else {
            fail += 1;
            msgs.push("FAIL  identity 未正确解析身份索引".into());
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过 identity 检查".into());
    }

    // ---- renaming must not desynchronise the map
    //
    // `normalize` renamed files and left every `PATH=` pointing at the old name.
    // The next `refresh` could not match them and read 228 missing files as 228
    // deletions, emptying 11 branches. Nobody noticed because `refresh` reported
    // a smaller number and nothing else.
    //
    // Two behaviours are pinned: the map follows the rename immediately, and a
    // rebuild that drops entries says which ones and flags a mass drop.
    if let Some(exe) = self_path() {
        let tmp = std::env::temp_dir().join(format!("abook-ren-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let recs = tmp.join("canonical/records");
        let _ = std::fs::create_dir_all(&recs);
        for i in 1..=2 {
            let _ = std::fs::write(
                recs.join(format!("AB2-C-00{i}.md")),
                format!(
                    "[CANONICAL_MEMORY]\nFORMAT=CANONICAL_MEMORY_V2\nID=AB2-C-00{i};PROJECT=p\nTITLE=标题{i}\nSTATUS=ACTIVE\n[/CANONICAL_MEMORY]\n[MEMORY_BODY]\n正文{i}\n[/MEMORY_BODY]\n"
                ),
            );
        }
        let _ = std::fs::write(
            tmp.join("canonical/Atlas.md"),
            "FORMAT=AGENTSBOOK_V2_EXPERIMENT\nLIBRARY=CANONICAL\n\nBRANCH=b1\nKEYS=k\nCOUNT=2\nMAXSIZE=1\nSUMMARY=s\nREAD_WHEN=r\nPATH=canonical/records/AB2-C-001.md,AB2-C-002.md\nREVISION=1\nSTATUS=ACTIVE\n",
        );
        let run = |args: &[&str]| -> String {
            match std::process::Command::new(&exe).args(args).output() {
                Ok(o) => format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => format!("{e}"),
            }
        };
        let t = tmp.to_string_lossy().to_string();
        let _ = run(&["--root", &t, "normalize", "--apply"]);
        // The map must name the *new* files immediately after the rename.
        //
        // Checking only "does refresh complain" was not enough: a rebuild that
        // drops a stale path re-adds the file to some guessed branch, so the
        // record count stays right and only the branch membership is silently
        // rewritten. That is exactly the damage seen on the real library -- 11
        // branches emptied while the total stayed 319. Assert on the map text.
        let atlas = std::fs::read_to_string(tmp.join("canonical/Atlas.md")).unwrap_or_default();
        let stale = atlas.contains("AB2-C-00");
        let _ = std::fs::remove_dir_all(&tmp);

        if !stale {
            pass += 1;
            msgs.push("PASS  改名同步更新地图，分支归属不会在下次 refresh 时被重猜".into());
        } else {
            fail += 1;
            msgs.push(
                "FAIL  改名后地图仍写着旧文件名——下一次 refresh 会把它当删除，\
再把记录重挂到猜测的分支上".into(),
            );
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过改名同步检查".into());
    }

    // ---- the two command inventories must agree, and every name must be checked
    //
    // `commands` and `describe` answer the same question -- "what can I type?" --
    // from two different tables, and they drifted: `commands` listed 70 names
    // while `describe` listed 56, because one walked the dispatch table and the
    // other walked the parameter table. The 14 aliases (`add`, `cat`, `find`,
    // `rm`, `map` …) worked when typed but were missing from the list, so a
    // caller enumerating it concluded they did not exist.
    //
    // The sharper consequence is a silent one. The per-command flag check looks
    // a command up in the parameter table; a command with no row there has
    // `params_of` return `None`, which the checker reads as "cannot validate
    // this one" and waves everything through. `search` had no row, so
    // `search --zzz` was accepted and ignored -- while every other command
    // rejected it. A missing row is not a cosmetic doc gap; it disables the
    // check the user asked for.
    {
        let cmds: Vec<String> = crate::known_commands()
            .iter()
            .filter(|n| !n.starts_with('-'))
            .map(|n| n.to_string())
            .collect();
        let missing: Vec<&String> = cmds
            .iter()
            .filter(|n| crate::command_spec(n.as_str()).is_none())
            .collect();
        // The tables are not the same length by design -- several aliases share
        // one parameter row -- so length equality is not the invariant. The two
        // invariants are: every typeable name is covered (no silent skip), and
        // no row describes a name that cannot be typed (no dead entry).
        let dangling: Vec<String> = crate::spec::names()
            .into_iter()
            .filter(|n| !cmds.contains(n))
            .collect();
        if !missing.is_empty() {
            fail += 1;
            msgs.push(format!(
                "FAIL  这些命令没有参数表条目，校验会被整个跳过: {}",
                missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        } else if !dangling.is_empty() {
            fail += 1;
            msgs.push(format!(
                "FAIL  参数表里有 {} 个名字无法输入: {}",
                dangling.len(),
                dangling.join(", ")
            ));
        } else {
            pass += 1;
            msgs.push(format!(
                "{} 个可输入名全部有参数表条目，无悬空条目",
                cmds.len()
            ));
        }
    }

    // ---- --root must actually select the library it names
    //
    // This is the defect that quietly invalidated every sandboxed probe in this
    // project. `Args::parse` stepped *over* flags that preceded the command but
    // never recorded them, so `abook --root <copy> list` dropped the flag,
    // `resolve_root` fell through to `~/AgentsBook`, and the command answered
    // confidently about the real library. Probes written as "--root <throwaway
    // copy>" were therefore writing to the live library while reporting success:
    // 146 records renamed, 11 map branches emptied, junk commits appended. All
    // exit code 0.
    //
    // A remembered path would pass a "returns something" check, so the sample
    // library must hold a record the real one cannot: an unmistakable title.
    if let Some(exe) = self_path() {
        let tmp = std::env::temp_dir().join(format!("abook-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let recs = tmp.join("canonical/records");
        let _ = std::fs::create_dir_all(&recs);
        let marker = "仅有此标记的记录ZZZ9";
        let _ = std::fs::write(
            recs.join("C-20260101T000000-abcdef01.md"),
            format!("[CANONICAL_MEMORY]\nFORMAT=CANONICAL_MEMORY_V2\nID=C-20260101T000000-abcdef01;PROJECT=p;TASK=t;SESSION=s\nKIND=DECISION\nTITLE={marker}\nSTATUS=ACTIVE\n[/CANONICAL_MEMORY]\n[MEMORY_BODY]\n正文\n[/MEMORY_BODY]\n"),
        );
        let run = |args: &[&str]| -> String {
            match std::process::Command::new(&exe).args(args).output() {
                Ok(o) => format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => format!("{e}"),
            }
        };
        let t = tmp.to_string_lossy().to_string();
        // Both orders must work: the flag before the command (the shape a
        // human writes) and the `--root=` form (the shape a script writes).
        let pre = run(&["--root", &t, "list"]);
        let eq = run(&[&format!("--root={t}"), "list"]);
        let _ = std::fs::remove_dir_all(&tmp);
        let hit_pre = pre.contains(marker);
        let hit_eq = eq.contains(marker);
        if hit_pre && hit_eq {
            pass += 1;
            msgs.push("PASS  --root 前置与 = 两种写法都真的切换了库".into());
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  --root 未生效（前置命中={hit_pre} 等号命中={hit_eq}）——\
命令读的仍是默认库"
            ));
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过 --root 检查".into());
    }

    // ---- a repairing command must not write unless asked to
    //
    // `refresh`, `recount`, `normalize` and `repair` all used to write the
    // moment they were invoked. That is the exact shape that hurt: someone runs
    // one to see what is wrong, and it fixes things instead. It happened for
    // real here -- a probe that walked 70 commands to answer "which ones read
    // --per-page" renamed 228 records, emptied 11 branches out of the map, and
    // appended fields to 39 more. Every one of those runs exited 0.
    //
    // Two directions are checked, because either alone is fooled: writing by
    // default (no --apply) and refusing to write when asked (--apply ignored).
    if let Some(exe) = self_path() {
        let tmp = std::env::temp_dir().join(format!("abook-dry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut ok = true;
        let mut detail = String::new();

        // Build a throwaway library holding one legacy-named record, so that
        // `normalize` has something to rename and `refresh`/`recount`/`repair`
        // have a map to disagree with.
        let recs = tmp.join("canonical/records");
        let _ = std::fs::create_dir_all(&recs);
        let legacy = "ID=AB2-C-001;PROJECT=p;TASK=t;SESSION=s\nTITLE=标题\n";
        let _ = std::fs::write(
            recs.join("AB2-C-001.md"),
            format!("[CANONICAL_MEMORY]\nFORMAT=CANONICAL_MEMORY_V2\n{legacy}STATUS=ACTIVE\n[/CANONICAL_MEMORY]\n[MEMORY_BODY]\n正文\n[/MEMORY_BODY]\n"),
        );
        let _ = std::fs::write(
            tmp.join("canonical/Atlas.md"),
            "FORMAT=ATLAS_V1\nBRANCH=x\nKEYS=k\nCOUNT=99\nMAXSIZE=1\nSUMMARY=s\nREAD_WHEN=r\nPATH=canonical/records/AB2-C-001.md\nREVISION=1\nSTATUS=ACTIVE\n",
        );
        let snap = |root: &std::path::Path| -> Vec<(String, u64)> {
            let mut v = Vec::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else {
                        v.push((
                            p.file_name().unwrap_or_default().to_string_lossy().to_string(),
                            std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
                        ));
                    }
                }
            }
            v.sort();
            v
        };
        let before = snap(&tmp);

        // Direction 1: no --apply, must not write.
        for c in ["refresh", "recount", "normalize", "repair"] {
            let o = std::process::Command::new(&exe)
                .args(["--root", &tmp.to_string_lossy(), c])
                .output();
            if let Ok(o) = o {
                let all = format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
                if all.contains("不接受 --apply") {
                    ok = false;
                    detail.push_str(&format!(" {c} 未登记 --apply"));
                }
            }
        }
        if snap(&tmp) != before {
            ok = false;
            detail.push_str(" 未加 --apply 时改动了文件");
        }

        // Direction 2: --apply must be accepted by each of the four, i.e. the
        // spec table has to list it. Asking `describe` avoids doing the write.
        for c in ["refresh", "recount", "normalize", "repair"] {
            let o = std::process::Command::new(&exe)
                .args(["describe", c])
                .output();
            let all = match o {
                Ok(o) => format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(_) => String::new(),
            };
            if !all.contains("--apply") {
                ok = false;
                detail.push_str(&format!(" {c} 的 describe 里没有 --apply"));
            }
        }
        // Direction 3: install's failure paths must not exit 0.
        //
        // `install` printed "警告: 装好的二进制自检未通过" and then returned 0, so a
        // script or CI saw success while holding a binary that fails its own
        // tests. The same held for a release so old it has no `selftest` at all.
        // Both now exit 5, matching how every other failed verification reports.
        // Checked structurally rather than by running install: downloading in a
        // test would hit the network, and the network is exactly what is flaky.
        // A warn-only path is a `println!` that falls through to the success
        // return; the fix is that each such arm returns non-zero first.
        {
            let src = std::fs::read_to_string("src/main.rs").unwrap_or_default();
            if !src.is_empty() {
                for (needle, label) in [
                    ("装好的二进制自检未通过", "自检失败"),
                    ("没有 selftest", "缺少 selftest"),
                ] {
                    let at = match src.find(needle) {
                        Some(i) => i,
                        None => {
                            ok = false;
                            detail.push_str(&format!(" 找不到「{label}」分支"));
                            continue;
                        }
                    };
                    // Look only inside THIS arm. A window of the next N bytes
                    // also covers the following arm, so removing the return from
                    // the first arm still "found" the second arm's return and the
                    // check passed on broken code -- a weak assertion is worse
                    // than none. The first non-blank line after the warning must
                    // be the non-zero return.
                    let _ = at;
                    let mut lines = src.lines().skip_while(|l| !l.contains(needle));
                    lines.next();
                    let next = lines.find(|l| !l.trim().is_empty()).unwrap_or("");
                    if !next.trim().starts_with("return Ok(5)") {
                        ok = false;
                        detail.push_str(&format!(
                            " 「{label}」只警告不返回非零（下一行是 {:?}）",
                            next.trim()
                        ));
                    }
                }

                // Direction 4: install must accept --token, because its own
                // message tells the user to pass it. The message said
                // "(--token 或 GITHUB_TOKEN)" while the spec table lacked the
                // flag, so following the advice produced USAGE and no download.
                let o = std::process::Command::new(&exe)
                    .args(["describe", "install"])
                    .output();
                let all = match o {
                    Ok(o) => format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    ),
                    Err(_) => String::new(),
                };
                if !all.contains("--token") {
                    ok = false;
                    detail.push_str(" install 的提示要求 --token，但 describe 里没有它");
                }
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);

        if ok {
            pass += 1;
            msgs.push("PASS  写类命令默认预演、--apply 已登记".into());
        } else {
            fail += 1;
            msgs.push(format!("FAIL  写类命令的写入边界不对:{detail}"));
        }

        // repair --only 必须真的限定范围。
        //
        // 这门的由来：`repair` 一执行就是"能修的全修"，于是想修 3 条记录的引用
        // 前缀，顺手给 248 条加 SIZE、给 242 条派生 KEYS——改的是没有版本控制的
        // 真库。加 `--only` 后必须验证它真的收窄了，而不只是日志少打几行：
        // 写盘按 `p.fields` 全量落地，只不记录的话字段照样被改。
        {
            let tmp = std::env::temp_dir().join(format!("abook-st-only-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            let mut detail = String::new();
            let mut ok = true;
            let dir0 = tmp.join("candidate").join("records");
            let _ = std::fs::create_dir_all(&dir0);
            if true {
                let f = dir0.join("N-20260101T000000-aaaaaaaa.md");
                let raw = "TITLE=限定测试\nREAD_WHEN=测试\nCONCEPTS=甲,乙\nBODY_MARK\n正文\n";
                if std::fs::write(&f, raw).is_ok() {
                    let run = |only: &str| -> String {
                        let _ = std::process::Command::new(&exe)
                            .args([
                                "repair", "--root", &tmp.to_string_lossy(), "--apply", "--only", only,
                            ])
                            .output();
                        std::fs::read_to_string(&f).unwrap_or_default()
                    };
                    // 只修 refs 类别，一条缺 SIZE/KEYS/FORMAT 的记录不该被补任何字段
                    let after_refs = run("refs");
                    let added = ["SIZE=", "KEYS=", "FORMAT="]
                        .iter()
                        .filter(|k| after_refs.contains(*k))
                        .count();
                    if added != 0 {
                        ok = false;
                        detail.push_str(&format!("--only refs 补了 {added} 个无关字段;"));
                    }
                    // 只修 size，应当只补 SIZE
                    let after_size = run("size");
                    if !after_size.contains("SIZE=") {
                        ok = false;
                        detail.push_str("--only size 没补 SIZE;");
                    }
                    if after_size.contains("KEYS=") || after_size.contains("FORMAT=") {
                        ok = false;
                        detail.push_str("--only size 越界补了别的;");
                    }
                } else {
                    ok = false;
                    detail.push_str("建不出测试记录;");
                }
            } else {
                ok = false;
                detail.push_str("建不出测试库;");
            }
            let _ = std::fs::remove_dir_all(&tmp);
            if ok {
                pass += 1;
                msgs.push("PASS  repair --only 真的只修该类别，不多动一个字段".into());
            } else {
                fail += 1;
                msgs.push(format!("FAIL  repair --only 没限定住范围:{detail}"));
            }
        }

        // 已晋升的候选不许再晋升一次。
        //
        // 晋升记录有两个时期的写法：新流程写末尾 `[PROMOTION]` 块，早期流程把
        // `PROMOTED_TO`/`PROMOTED_AT` 直接写在头部字段。只看末尾块时，一条头部
        // 已标 `PROMOTED_TO=C-...` 的老候选会被当成没晋升过再晋升一遍——正式库
        // 于是出现两条标题相同、`PROMOTED_FROM` 也相同的记录。这事真发生过，
        // 所以两种写法都必须拦住。
        {
            let tmp = std::env::temp_dir().join(format!("abook-st-promo-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            let mut detail = String::new();
            let mut ok = true;
            let cdir = tmp.join("candidate").join("records");
            let _ = std::fs::create_dir_all(&cdir);
            let _ = std::fs::create_dir_all(tmp.join("canonical").join("records"));
            let mk = |name: &str, extra: &str| -> std::path::PathBuf {
                let f = cdir.join(format!("{name}.md"));
                let _ = std::fs::write(
                    &f,
                    format!(
                        "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID={name}\nTITLE=晋升测试\n\
                         KIND=DECISION\nMATURITY=VERIFIED\nAUTHORITY=USER_EXPLICIT\nSUMMARY=摘要\n\
                         KEYS=甲\nREAD_WHEN=测试\nSIZE=SMALL\n{extra}[/CANDIDATE_MEMORY]\n\
                         [MEMORY_BODY]\n正文内容足够长可以接受\n[/MEMORY_BODY]\n"
                    ),
                );
                f
            };
            // 头部已带 PROMOTED_TO 的老候选
            mk(
                "N-20260101T000000-cccccccc",
                "PROMOTED_TO=C-20260101T010000-11111111\nPROMOTED_AT=2026-01-01T01:00:00+08:00\n",
            );
            let o = std::process::Command::new(&exe)
                .args([
                    "promote",
                    "--root",
                    &tmp.to_string_lossy(),
                    "N-20260101T000000-cccccccc",
                    "--approve",
                ])
                .output();
            if let Ok(o) = o {
                let all = format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
                if !all.contains("此前晋升过") {
                    ok = false;
                    detail.push_str("头部已有 PROMOTED_TO 的候选未被拦住;");
                }
            }
            // 正式库里不能多出记录
            let n = std::fs::read_dir(tmp.join("canonical").join("records"))
                .map(|d| d.count())
                .unwrap_or(0);
            if n != 0 {
                ok = false;
                detail.push_str(&format!("拦下后仍写了 {n} 条正式记录;"));
            }
            // 未晋升的候选仍必须能晋升（不能把正常路径一起堵死）
            mk("N-20260101T000000-dddddddd", "");
            let o2 = std::process::Command::new(&exe)
                .args([
                    "promote",
                    "--root",
                    &tmp.to_string_lossy(),
                    "N-20260101T000000-dddddddd",
                    "--approve",
                ])
                .output();
            let n2 = std::fs::read_dir(tmp.join("canonical").join("records"))
                .map(|d| d.count())
                .unwrap_or(0);
            if n2 != 1 {
                ok = false;
                if let Ok(o2) = o2 {
                    detail.push_str(&format!(
                        "正常晋升被堵死({n2} 条) {};",
                        crate::util::truncate_chars(&String::from_utf8_lossy(&o2.stderr), 60)
                    ));
                }
            }
            let _ = std::fs::remove_dir_all(&tmp);
            if ok {
                pass += 1;
                msgs.push("PASS  已晋升的候选（头部或尾块任一写法）不能再晋升一次".into());
            } else {
                fail += 1;
                msgs.push(format!("FAIL  重复晋升没拦住:{detail}"));
            }
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过写边界检查".into());
    }

    // ---- invocation shapes a caller writes without reading the manual
    //
    // Both of these were rejected while every unit test stayed green, because
    // each defect lived in argument parsing rather than in a command:
    //   `abook --root ~/AgentsBook state`  -> "unknown command: --root"
    //   `abook list --help`                 -> "list 不接受 --help"
    // The second one is the sharper trap: `dispatch` exempts `--help`, but the
    // per-command validator ran first and never let dispatch see it.
    if let Some(exe) = self_path() {
        let root = crate::util::resolve_root(&crate::util::Args::default());
        let run = |args: &[&str]| -> (String, i32) {
            match std::process::Command::new(&exe).args(args).output() {
                Ok(o) => (
                    format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    ),
                    o.status.code().unwrap_or(-1),
                ),
                Err(e) => (format!("{e}"), -1),
            }
        };
        // `state` exits 5 when it finds consistency problems, which is a
        // legitimate answer, so it cannot stand in for "did the pre-command
        // global flag work". `version` always exits 0 and prints a banner.
        //
        // The expected banner is built from VERSION, never spelled out. These
        // two assertions used to hardcode "abook 1.0.0", so the moment the
        // version was made to follow the release tag both flipped to FAIL and
        // the real question -- did the flag reach the command? -- was buried
        // under a stale literal.
        let banner = format!("abook {}", crate::VERSION);
        let (pre, pre_rc) = run(&["--root", &root.to_string_lossy(), "version"]);
        let (per_help, help_rc) = run(&["list", "--help"]);
        let (bare, bare_rc) = run(&["--root"]);
        let ok = pre_rc == 0
            && pre.contains(&banner)
            && help_rc == 0
            && per_help.contains(&banner)
            && bare_rc == 2
            && bare.contains("需要一个值")
            && !bare.contains("未知命令");
        if ok {
            pass += 1;
            msgs.push("PASS  前置全局参数、单命令 --help、缺值提示三种写法都正确".into());
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  前置 --root rc={pre_rc}(应0) / list --help rc={help_rc}(应0) / \
--root 缺值 rc={bare_rc}(应2) 提示「{}」",
                bare.lines().next().unwrap_or("")
            ));
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过调用形状检查".into());
    }

    // ---- commit ordering reads the recorded time, not the filename
    //
    // Two defects hid here at once. `history` sorted by filename, so the old
    // `0001-` sequence landed after every timestamp name and `-t-1` sorted
    // before `-t` (44 of 76 rows misplaced). And the timestamp reader dropped
    // the UTC offset, so `...Z` and `...+08:00` rows -- both present in the
    // live library -- came out eight hours apart.
    {
        let a = |s: &str| crate::util::parse_stamp(s);
        let z = a("2026-10-23T04:05:50Z");
        let plus8 = a("2026-10-23T12:05:50+08:00");
        let bracket = a("2026-10-23T12:05:50+08:00[Asia/Shanghai]");
        let spaced = a("2026-09-27 13:15:47 +08:00");
        let compact = a("20260927T131547");
        // Same instant expressed three ways must compare equal.
        let zones_agree = z.is_some() && z == plus8 && plus8 == bracket;
        let other_forms = spaced.is_some() && compact.is_some();
        // An entry whose time field is absent must still be placed, not dropped.
        let dir = std::env::temp_dir().join(format!("abook-ct-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let named = dir.join("20260102T030405-slug.md");
        let _ = std::fs::write(&named, "no time field here");
        let from_name = crate::util::commit_time(&named, "no time field here");
        let _ = std::fs::remove_dir_all(&dir);
        let name_ok = from_name > 0 && Some(from_name) == a("20260102T030405");
        if zones_agree && other_forms && name_ok {
            pass += 1;
            msgs.push("PASS  提交时间：时区归一、五种写法可解、无字段时回退文件名".into());
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  提交时间：Z/+08:00/带方括号三者等同={zones_agree} \
其它写法={other_forms} 文件名回退={name_ok}"
            ));
        }
    }

    // ---- every advertised name must actually dispatch
    //
    // The check above compares two hand-written constants — the help prose and
    // the name list — so deleting a real dispatch arm leaves both intact and it
    // still reports PASS. That is exactly what happened to `leftover`: removed
    // from the match, self-check stayed 104/104 green. Driving the binary is the
    // only way to observe the third thing that can disagree: the match itself.
    //
    // `--zzz` is rejected by argument validation before dispatch runs, so a
    // live command answers "不接受/无法识别" while a phantom answers "未知命令".
    // `selftest` is excluded because probing it would recurse forever.
    if let Some(exe) = self_path() {
        let mut phantom: Vec<String> = Vec::new();
        let mut probed = 0usize;
        for name in crate::known_commands() {
            // Dash-prefixed names are flags (`--version`), not commands, so
            // probing them measures nothing; `--help --zzz` even reports the
            // phantom for `--zzz` rather than for the name under test.
            if matches!(*name, "selftest" | "version") || name.starts_with('-') {
                continue;
            }
            probed += 1;
            match std::process::Command::new(&exe).arg(name).arg("--zzz").output() {
                Ok(o) => {
                    let text = format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    );
                    if text.contains("未知命令") {
                        phantom.push(name.to_string());
                    }
                }
                Err(e) => phantom.push(format!("{name}(无法执行: {e})")),
            }
        }
        if phantom.is_empty() {
            pass += 1;
            if verbose {
                msgs.push(format!("PASS  commands 列出的 {probed} 条命令都能真的分派"));
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  commands 列出但实际分派不到的命令: {}",
                phantom.join(", ")
            ));
        }
    }

    // ---- CI config matches the code
    // ---- `commands` output is exactly the dispatch table
    // Docs, CI and any AI caller are pointed at `abook commands` as the
    // authoritative list, so it must not silently disagree with the binary
    // that actually runs. `help` was missing from the table while working,
    // which meant the docs check called a real command phantom.
    {
        let listed = crate::known_commands()
            .iter()
            .copied()
            .filter(|c| !c.starts_with('-'))
            .count();
        let printed = {
            let mut n = 0usize;
            let mut seen: Vec<&str> = Vec::new();
            for c in crate::known_commands().iter().copied().filter(|c| !c.starts_with('-')) {
                if !seen.contains(&c) {
                    seen.push(c);
                    n += 1;
                }
            }
            n
        };
        if listed == printed && listed > 40 {
            pass += 1;
            if verbose {
                msgs.push(format!("PASS  `commands` 输出的 {listed} 条与 dispatch 表一致"));
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  `commands` 输出 {printed} 条，dispatch 表 {listed} 条，两者不一致"
            ));
        }
    }

    // Three separate drifts have already shipped from here: a duplicate YAML
    // key that made the whole workflow invalid (0 jobs), a target that CI
    // built but Rust never mapped, and a compiler name that Configure and
    // Verify each spelled differently.
    {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let script = root.join("tools").join("check_workflow.py");
        // A released binary runs on a machine that has no checkout, so
        // CARGO_MANIFEST_DIR points nowhere. That is not a defect in the tool:
        // this check audits the repo, so outside the repo it is SKIP, never
        // FAIL. Marking it FAIL once made every downloaded binary look broken.
        if !script.exists() {
            skip += 1;
            if verbose {
                msgs.push("SKIP  workflow 校验（不在仓库内，无 tools/check_workflow.py）".to_string());
            }
        } else if ok_workflow(&script, root) {
            pass += 1;
            if verbose {
                msgs.push("PASS  workflow 与代码一致（目标清单、linker 解析）".to_string());
            }
        } else {
            fail += 1;
            msgs.push("FAIL  workflow 与代码不一致（python3 tools/check_workflow.py 看详情）".to_string());
        }
    }

    // ---- the operator doc's commands all run
    // The doc is meant to replace the memory-library chapter of the prompt, so
    // a command it advertises has to work. The checker copies the library and
    // runs each line in a fresh copy (a preceding `move` would otherwise make
    // the next `delete` report NOT_FOUND and look like a tool bug).
    {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let script = root.join("tools").join("check_docs.py");
        if !script.exists() {
            skip += 1;
            if verbose {
                msgs.push("SKIP  文档实测（不在仓库内，无 tools/check_docs.py）".to_string());
            }
        } else if ok_docs(&script, root) {
            pass += 1;
            if verbose {
                msgs.push("PASS  操作文档里每条命令都真能跑".to_string());
            }
        } else {
            fail += 1;
            msgs.push("FAIL  操作文档里有命令跑不通（python3 tools/check_docs.py 看详情）".to_string());
        }
    }

    // ---- every real command is discoverable
    // Aliases are fine, but a genuine capability that the help never mentions
    // is undiscoverable: `commit` writes a change record and was invisible.
    {
        let help = crate::help_text();
        let mut listed: Vec<String> = Vec::new();
        for line in help.lines() {
            let t = line.trim_start();
            if !line.starts_with("  ") || t.starts_with(' ') || t.starts_with('-') {
                continue;
            }
            let mut words = t.split_whitespace();
            let mut w = words.next().unwrap_or("");
            // The "start here" section spells entries as `abook selftest`.
            if w == "abook" {
                w = words.next().unwrap_or("");
            }
            if w.chars().all(|c| c.is_ascii_lowercase() || c == '-') && w.len() > 1 {
                listed.push(w.to_string());
            }
        }
        // Spellings that intentionally share another command's help entry.
        const ALIASES: &[&str] = &[
            "cp", "ls", "rm", "find", "where", "add", "c", "n", "candidate",
            "canonical", "self-install", "self-update", "index-search", "repair",
        ];
        let hidden: Vec<&str> = crate::known_commands()
            .iter()
            .copied()
            .filter(|c| !c.starts_with('-') && c.len() > 1)
            .filter(|c| !listed.iter().any(|l| l == c))
            .filter(|c| !ALIASES.contains(c))
            .collect();
        if hidden.is_empty() {
            pass += 1;
            if verbose {
                msgs.push("PASS  所有真实命令都能在帮助里查到".to_string());
            }
        } else {
            fail += 1;
            msgs.push(format!("FAIL  这些命令存在但帮助没提: {}", hidden.join(", ")));
        }
    }

    // ---- pagination sentinels
    // `--per-page 0` means "no pagination". It was clamped to 1, and the
    // `usize::MAX` sentinel then overflowed `total + per - 1`, so asking for
    // everything returned a single row or nothing at all -- a wrong answer that
    // looks like a small library. The default also disagreed with the docs.
    {
        let mk = |args: &[&str]| {
            let mut v: Vec<String> = vec!["abook".into()];
            v.extend(args.iter().map(|s| s.to_string()));
            crate::util::Args::parse(&v).unwrap()
        };
        let a0 = mk(&["list", "--per-page", "0"]);
        let p0 = crate::paging::Page::from_args(&a0, 507);
        let dflt = crate::paging::Page::from_args(&mk(&["list"]), 507);
        let a5 = mk(&["list", "--per-page", "5"]);
        let p5 = crate::paging::Page::from_args(&a5, 507);
        let ok = p0.per == usize::MAX && p0.start() == 0 && p0.end() == 507
            && dflt.per == 200
            && p5.per == 5 && p5.pages == 102;
        if ok {
            pass += 1;
            if verbose {
                msgs.push("PASS  分页：per-page 0 返回全部、默认 200、5 条分 102 页".to_string());
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  分页语义错误：per0={} start0={} end0={} 默认={} per5={} pages5={}",
                p0.per, p0.start(), p0.end(), dflt.per, p5.per, p5.pages
            ));
        }
    }

    // ---- preflight really fails loudly
    // A preflight that prints "failed" but exits 0 lets CI go green on a broken
    // tree. It must be proven, not assumed -- and the proof has to run the real
    // ending branch, since relaxing `exit 1` to `exit 0` is exactly the bug.
    {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let script = root.join("tools").join("preflight.sh");
        if !script.exists() {
            skip += 1;
            if verbose {
                msgs.push("SKIP  预检退出码（不在仓库内，无 tools/preflight.sh）".to_string());
            }
        } else if ok_preflight(&script) {
            pass += 1;
            if verbose {
                msgs.push("PASS  预检失败会以非零退出（--verify-exit）".to_string());
            }
        } else {
            fail += 1;
            msgs.push("FAIL  预检失败却返回 0，CI 会放行".to_string());
        }
    }

    // ---- dry-run output stays short
    // A dry run once printed the whole record. For a model caller that spent
    // context on content it already had and buried the outcome line.
    {
        // Text matching was wrong here: the --verbose branch legitimately
        // prints the body, so any substring test sees it. Test the behaviour
        // instead -- a dry run must emit few lines, and --verbose must emit
        // more.
        let ok = {
            let mut rec = crate::rec::Record::parse_str(
                "[CANDIDATE_MEMORY]\nTITLE=t\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\nbodyline1\nbodyline2\nbodyline3\n[/MEMORY_BODY]\n",
                std::path::Path::new("d.md"),
                crate::rec::Lib::Candidate,
            )
            .unwrap();
            let short = crate::cmd_edit::render_dry_run_for_test(&rec, "op", "x.md", false);
            let long = crate::cmd_edit::render_dry_run_for_test(&rec, "op", "x.md", true);
            short.lines().count() <= 3 && long.lines().count() > short.lines().count()
        };
        if ok {
            pass += 1;
            if verbose {
                msgs.push("PASS  dry-run 只输出摘要，全文要 --verbose".to_string());
            }
        } else {
            fail += 1;
            msgs.push("FAIL  dry-run 又把全文打出来了".to_string());
        }
    }

    // ---- every vault sub-command the help lists must exist
    // The top-level check cannot see these: `vault` is one command, so a
    // documented-but-missing sub-command slips through. Four did.
    {
        let help = crate::help_text();
        let known = crate::cmd_vault::known_subcommands();
        let mut listed: Vec<String> = Vec::new();
        for line in help.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("vault ") {
                if let Some(word) = rest.split_whitespace().next() {
                    if word.chars().all(|c| c.is_ascii_lowercase()) && word.len() > 1 {
                        listed.push(word.to_string());
                    }
                }
            }
            // The summary line spells them space-separated after a colon.
            // Scope it to the vault line itself: `skill` now has its own
            // `子命令：` summary, and matching on the colon alone swept its
            // subcommands into the vault list and reported six of them missing.
            if let Some(idx) = t.find("子命令：") {
                let head = t[..idx].trim();
                if head == "vault" {
                    for word in t[idx + "子命令：".len()..].split_whitespace() {
                        if word.chars().all(|c| c.is_ascii_lowercase()) && word.len() > 1 {
                            listed.push(word.to_string());
                        }
                    }
                }
            }
        }
        listed.sort();
        listed.dedup();
        let missing: Vec<&String> = listed.iter().filter(|c| !known.contains(&c.as_str())).collect();
        if missing.is_empty() && !listed.is_empty() {
            pass += 1;
            if verbose {
                msgs.push(format!("PASS  保险箱子命令齐全（核对 {} 个）", listed.len()));
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  保险箱子命令缺失: {}",
                if missing.is_empty() { "帮助段没解析到".to_string() }
                else { missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") }
            ));
        }
    }

    // ---- vault seeds must be independent
    // Derived parameters come from the seed, so near-identical seeds make every
    // entry share almost the same parameters. A same-second rekey used to
    // differ only in the last two digits.
    {
        let seeds: Vec<String> = (0..8).map(|_| crate::vault::new_seed()).collect();
        let all_ok = seeds.iter().all(|s| {
            s.len() == 12 && s.chars().all(|c| c.is_ascii_digit())
        });
        let distinct = {
            let mut v = seeds.clone();
            v.sort();
            v.dedup();
            v.len() == seeds.len()
        };
        // the first 8 digits must not be shared, or parameters collapse together
        let prefix_distinct = {
            let mut v: Vec<String> = seeds.iter().map(|s| s[..8].to_string()).collect();
            v.sort();
            v.dedup();
            v.len() == seeds.len()
        };
        if all_ok && distinct && prefix_distinct {
            pass += 1;
            if verbose {
                msgs.push("PASS  保险箱种子：12 位数字、互不相同、前缀不撞".to_string());
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  保险箱种子：格式={all_ok} 唯一={distinct} 前缀唯一={prefix_distinct}"
            ));
        }
    }

    // ---- header values: read multi-line, write single-line
    // Two records in the real library carry an 18-line SUMMARY, and one ID line
    // embeds PROJECT/TASK/SESSION after semicolons. Reading must tolerate both;
    // writing must never produce them, or every line-based reader breaks.
    {
        let mut good = 0;
        let total = 3;

        // reading: continuation lines belong to the preceding field
        let multi = "[CANDIDATE_MEMORY]\nTITLE=t\nSUMMARY=line one\nline two\nline three\nREAD_WHEN=w\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\nbody\n[/MEMORY_BODY]\n";
        let pp = std::path::Path::new("x.md");
        if let Ok(r) = crate::rec::Record::parse_str(multi, pp, crate::rec::Lib::Candidate) {
            if r.get("SUMMARY").contains("line two") && r.get("READ_WHEN") == "w" {
                good += 1;
            }
        }

        // reading: embedded fields in one line are split out
        let embedded = "[CANDIDATE_MEMORY]\nID=C-1;PROJECT=pj;TASK=tk;SESSION=ss\nKIND=DECISION\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\nbody\n[/MEMORY_BODY]\n";
        if let Ok(r2) = crate::rec::Record::parse_str(embedded, std::path::Path::new("y.md"), crate::rec::Lib::Candidate) {
            if r2.get("ID") == "C-1" && r2.get("PROJECT") == "pj" && r2.get("TASK") == "tk" {
                good += 1;
            }
        }

        // writing: go through Record::set, not through the helper. Testing the
        // helper alone would pass even if set() never called it.
        let mut rw = crate::rec::Record::parse_str(
            "[CANDIDATE_MEMORY]\nTITLE=t\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\nb\n[/MEMORY_BODY]\n",
            std::path::Path::new("z.md"),
            crate::rec::Lib::Candidate,
        )
        .unwrap();
        rw.set("SUMMARY", "line one\nline two\n\tline three");
        if rw.get("SUMMARY") == "line one line two line three" {
            good += 1;
        }

        if good == total {
            pass += 1;
            if verbose {
                msgs.push("PASS  头部字段：跨行可读、嵌入字段可拆、写入强制单行".to_string());
            }
        } else {
            fail += 1;
            msgs.push(format!("FAIL  头部字段：{total} 项中只有 {good} 项正确"));
        }
    }

    // ---- platform table completeness
    // Every triple advertised by `detect` must also be built by CI. The list
    // below is the CI matrix; a mismatch means an install would 404.
    const CI_TARGETS: &[&str] = &[
        "aarch64-linux-android",
        "armv7-linux-androideabi",
        "x86_64-linux-android",
        "i686-linux-android",
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
        "armv7-unknown-linux-musleabihf",
        "i686-unknown-linux-musl",
        "riscv64gc-unknown-linux-gnu",
        "loongarch64-unknown-linux-gnu",
        "x86_64-pc-windows-gnullvm",
        "i686-pc-windows-gnullvm",
        "aarch64-pc-windows-gnullvm",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ];
    const PLATFORMS: &[(&str, &str)] = &[
        ("android", "aarch64"),
        ("android", "arm"),
        ("android", "x86_64"),
        ("android", "x86"),
        ("linux", "x86_64"),
        ("linux", "aarch64"),
        ("linux", "arm"),
        ("linux", "x86"),
        ("linux", "riscv64"),
        ("linux", "loongarch64"),
        ("windows", "x86_64"),
        ("windows", "x86"),
        ("windows", "aarch64"),
        ("macos", "x86_64"),
        ("macos", "aarch64"),
    ];
    for (os, arch) in PLATFORMS {
        match crate::target_triple(os, arch) {
            Some(t) if CI_TARGETS.contains(&t) => {
                pass += 1;
                if verbose {
                    msgs.push(format!("PASS  平台 {os}/{arch} → {t}"));
                }
            }
            Some(t) => {
                fail += 1;
                msgs.push(format!("FAIL  平台 {os}/{arch} → {t} 不在 CI 矩阵里，安装会 404"));
            }
            None => {
                fail += 1;
                msgs.push(format!("FAIL  平台 {os}/{arch} 无法识别"));
            }
        }
    }

    // ---- Write fidelity -------------------------------------------------
    // Rewriting Global.md / Index.md / Atlas.md must not disturb lines the
    // command did not intend to change. These three checks exist because each
    // one was a real defect measured on the shipped library: a write dropped
    // three blank lines from Index.md, Atlas re-rendering appended the title
    // line to the header again on every write, and MAXSIZE was overwritten
    // with the newest record's size instead of the largest.
    {
        let mut bf = crate::book::BookFile {
            meta: Vec::new(),
            meta_rows: Vec::new(),
            content: "## Active Routes\n\na｜b｜c\n\n".into(),
            trailing: String::new(),
            head_raw: "[MEMORY_BOOK_V2]\nX=1\n\n\n[/MEMORY_BOOK_V2]\n\n[MEMORY_CONTENT]\n".into(),
            tail_raw: "[/MEMORY_CONTENT]\n".into(),
        };
        let before = bf.render();
        bf.upsert_route("a", "b", "c"); // same row, no change
        let ok = bf.render() == before;
        if ok {
            pass += 1;
            if verbose {
                msgs.push("PASS  无改动时 Index.md 渲染逐字节相同".into());
            }
        } else {
            fail += 1;
            msgs.push("FAIL  无改动时 Index.md 渲染改变了字节".into());
        }
    }
    {
        // Blank lines inside the meta block must survive a write.
        let mut bf = crate::book::BookFile {
            meta: Vec::new(),
            meta_rows: Vec::new(),
            content: "## Active Routes\n\na｜b｜c\n".into(),
            trailing: String::new(),
            head_raw: "[MEMORY_BOOK_V2]\nX=1\n\n\nY=2\n[/MEMORY_BOOK_V2]\n\n[MEMORY_CONTENT]\n".into(),
            tail_raw: "[/MEMORY_CONTENT]\n".into(),
        };
        bf.upsert_route("z", "zz", "zzz");
        let r = bf.render();
        let ok = r.contains("X=1\n\n\nY=2\n") && r.contains("z｜zz｜zzz");
        if ok {
            pass += 1;
            if verbose {
                msgs.push("PASS  写路由保留头部原有空行".into());
            }
        } else {
            fail += 1;
            msgs.push("FAIL  写路由吞掉了头部空行".into());
        }
    }
    {
        // A bare trailing `PATH=` is an end-of-card marker, not a blank line.
        let src = "# T\n\nFORMAT=X\nRECORD_COUNT=1\nBRANCH=b\nKEYS=k\nCOUNT=1\nPATH=a/1.md\nPATH=\n";
        let at = crate::atlas::Atlas::load_str(src, "");
        let ok = match at {
            Ok(a) => a.render() == src,
            Err(_) => false,
        };
        if ok {
            pass += 1;
            if verbose {
                msgs.push("PASS  Atlas 往返保留末尾空 PATH= 标记".into());
            }
        } else {
            fail += 1;
            msgs.push("FAIL  Atlas 往返丢失末尾空 PATH= 标记".into());
        }
    }

    // --- Atlas: mixed PATH spellings survive a rebuild -----------------
    // The shipped library writes both `canonical/records/x.md` and a bare
    // `x.md`, sometimes inside the same card (185 full / 136 bare). `paths()`
    // expands and `set_paths()` abbreviates, so routing an edit through them
    // silently re-spells untouched elements. Verbatim read+write is the only
    // form that keeps an untouched card byte-identical.
    {
        // Real files open with `# <relative path>` then a blank line; the
        // fixture mirrors that so the round trip compares like with like.
        let body = "# canonical/Atlas.md\n\nBRANCH=a\nKEYS=k\nCOUNT=3\nPATH=canonical/records/x.md,y.md\nREVISION=1\nSTATUS=ACTIVE\n\nBRANCH=b\nKEYS=k\nCOUNT=1\nPATH=z.md\nREVISION=1\nSTATUS=ACTIVE\n";
        let mut at = crate::atlas::Atlas::load_str(body, "canonical/Atlas.md").expect("atlas");
        let raw: Vec<String> = at.branches.iter().flat_map(|b| b.raw_paths()).collect();
        let want = vec![
            "canonical/records/x.md".to_string(),
            "y.md".to_string(),
            "z.md".to_string(),
        ];
        if raw != want {
            fail += 1;
            msgs.push(format!("FAIL  raw_paths 返回 {raw:?}，应为 {want:?}"));
        } else if at.render() != body {
            fail += 1;
            msgs.push(format!("FAIL  Atlas 无损往返改变了字节\n  in ={:?}\n  out={:?}", body, at.render()));
        } else {
            let idx = at.branches.iter().position(|b| b.name() == "a").unwrap();
            let mut ps = at.branches[idx].raw_paths();
            ps.push("w.md".into());
            at.branches[idx].set_paths_raw(&ps);
            let line = at.branches[idx].get("PATH");
            if line != "canonical/records/x.md,y.md,w.md" {
                fail += 1;
                msgs.push(format!("FAIL  新增元素改写了他项写法: {line}"));
            } else {
                pass += 1;
                msgs.push("PASS  Atlas 混用写法往返保真且新增不改写他项".into());
            }
        }
    }

    // --- a commit must read back as the library's own format -------------
    //
    // The first version of the commit writer emitted `ID/TIME/KIND` and a bare
    // `Z` timestamp. Those files parse as commits, so nothing complained, yet
    // they were a different dialect from the 107 already in the library: no
    // OPERATION, no SCOPE, no SUMMARY, and a time three spellings away from
    // what every reader expects. A proxy that forks the format is worse than
    // no proxy, so the writer now refuses anything that would not read back.
    {
        let good = "[COMMIT]\nFORMAT=AGENTSBOOK_V2_EXPERIMENT\nCOMMIT_ID=x\nID=x\n\
OPERATION=PATCH\nKIND=set-field\nLIBRARY=CANDIDATE\nTIME=2026-10-23T13:59:47+08:00[Asia/Shanghai]\n\
TARGET=candidate/records/a.md\nSCOPE=仅改动 1 条记录\nSUMMARY=改了一个字段\n[/COMMIT]";
        match vet_commit_for_test("x.md", good) {
            Ok(()) => {
                pass += 1;
                msgs.push("PASS  合法的变更记录通过写入前校验".into());
            }
            Err(e) => {
                fail += 1;
                msgs.push(format!("FAIL  合法变更记录被拒: {e}"));
            }
        }

        // Each of these is a defect the writer could plausibly grow. Every one
        // must be refused rather than written.
        let cases: Vec<(&str, String)> = vec![
            (
                "缺 OPERATION",
                good.replace("OPERATION=PATCH\n", ""),
            ),
            (
                "缺 SUMMARY",
                good.replace("SUMMARY=改了一个字段", "SUMMARY="),
            ),
            // Must use a NON-required key. Lower-casing `OPERATION` also
            // removes the required field, so the "missing field" rule would
            // catch it first and the case would never exercise the
            // upper-case rule at all -- it would stay green even if the
            // upper-case rule were deleted.
            (
                "键名小写",
                good.replace("KIND=set-field", "kind=set-field"),
            ),
            (
                "键名重复",
                good.replace("KIND=set-field", "KIND=set-field\nKIND=again"),
            ),
            (
                "值里含分号",
                good.replace("SUMMARY=改了一个字段", "SUMMARY=改了一个;字段"),
            ),
            (
                "时间无法解析",
                good.replace("2026-10-23T13:59:47+08:00[Asia/Shanghai]", "昨天下午"),
            ),
            (
                "容器标记错",
                good.replace("[COMMIT]", "[COMMIT_RECORD]"),
            ),
            (
                "值全空",
                good.replace("TIME=2026-10-23T13:59:47+08:00[Asia/Shanghai]", "TIME= "),
            ),
        ];
        let mut missed: Vec<&str> = Vec::new();
        for (why, text) in &cases {
            if vet_commit_for_test("x.md", text).is_ok() {
                missed.push(why);
            }
        }
        if missed.is_empty() {
            pass += 1;
            msgs.push(format!("PASS  写入前校验拦下全部 {} 类畸形变更记录", cases.len()));
        } else {
            fail += 1;
            msgs.push(format!("FAIL  这些畸形变更记录被放行: {}", missed.join("、")));
        }
    }

    // --- export honours the format it was asked for ----------------------
    //
    // `--format` was a free-form string with no enum check, and anything that
    // was not `jsonl` fell through to the markdown branch. `--format json`
    // therefore printed `[CANONICAL_MEMORY]` blocks: the caller asked for JSON,
    // got prose, and every layer above it accepted the text because the command
    // exited 0. A format the tool cannot produce must be a usage error.
    if let Some(exe) = self_path() {
        let live = crate::util::resolve_root(&crate::util::Args::default());
        let mut bad: Vec<String> = Vec::new();
        let probe = |args: &[&str]| -> Option<(i32, String, String)> {
            let out = std::process::Command::new(&exe)
                .args(args)
                .args(["--root", &live.display().to_string()])
                .output()
                .ok()?;
            Some((
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            ))
        };
        if let Some((_, out, _)) = probe(&["export", "--format", "json", "--page", "1", "--per-page", "2"]) {
            let t = out.trim();
            // Whole-document parse plus "not the empty array": the shape check
            // already walks brackets and strings, so a non-empty array that
            // parses is exactly the claim being made.
            let ok = t.starts_with('[') && t.len() > 2 && json_one_ok(t);
            if !ok {
                bad.push(format!("--format json 输出不像数组: {}", crate::util::truncate_chars(t, 40)));
            }
        }
        if let Some((rc, _, err)) = probe(&["export", "--format", "xml"]) {
            if rc == 0 {
                bad.push("--format xml 被接受（未知格式必须报用法错误）".to_string());
            } else if !err.is_empty() && rc != 2 {
                bad.push(format!("--format xml 退出码 {rc}，应为 2"));
            }
        }
        if let Some((_, out, _)) = probe(&["export", "--format", "jsonl", "--page", "1", "--per-page", "2", "--json"]) {
            let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
            if lines.len() != 2 {
                bad.push(format!("--format jsonl 应出 2 行，实得 {}", lines.len()));
            }
            if !lines.iter().all(|l| json_one_ok(l.trim())) {
                bad.push("--format jsonl 有行不是独立 JSON".to_string());
            }
        }
        if bad.is_empty() {
            pass += 1;
            msgs.push("PASS  export 忠实于所请求的格式（json 数组 / jsonl 逐行 / 未知格式报错）".to_string());
        } else {
            fail += 1;
            msgs.push(format!("FAIL  export 格式处理: {}", bad.join("; ")));
        }
    }

    // --- cat prints the body, show prints the record ---------------------
    //
    // The two names share one handler, which is how `cat` came to print headers
    // plus body while the help screen promised "只输出正文". A pipe feeding
    // `cat` into a summariser got the whole file. The check compares line
    // counts rather than hunting for a marker string, because a body may
    // legitimately contain the word TITLE.
    if let Some(exe) = self_path() {
        let root = crate::util::resolve_root(&crate::util::Args::default());
        let first = first_record_arg(&root);
        let mut bad: Vec<String> = Vec::new();
        let probe = |args: &[&str]| -> Option<(i32, String, String)> {
            let out = std::process::Command::new(&exe)
                .args(args)
                .arg("--root")
                .arg(&root)
                .output()
                .ok()?;
            Some((
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            ))
        };
        if let Some((rc, cat_out, _)) = probe(&["cat", &first]) {
            if rc == 0 {
                if let Some((_, show_out, _)) = probe(&["show", &first]) {
                    let cl = cat_out.lines().filter(|l| !l.trim().is_empty()).count();
                    let sl = show_out.lines().filter(|l| !l.trim().is_empty()).count();
                    if cl >= sl {
                        bad.push(format!("cat 输出 {cl} 行，show {sl} 行：cat 必须更短"));
                    }
                    if cat_out.contains("FORMAT=CANONICAL_MEMORY")
                        || cat_out.contains("FORMAT=CANDIDATE_MEMORY")
                    {
                        bad.push("cat 输出了头部字段".to_string());
                    }
                }
            } else {
                bad.push(format!("cat {first} 退出码 {rc}"));
            }
        }
        if let Some((rc, out, _)) = probe(&["describe", "set", "--json"]) {
            if rc != 0 {
                bad.push(format!("describe set --json 退出码 {rc}"));
            } else if !out.contains("\"usage\":\"<记录> <字段> <值>\"") {
                bad.push(format!(
                    "describe set 没给出位置参数: {}",
                    crate::util::truncate_chars(out.trim(), 90)
                ));
            }
        }
        if let Some((_, out, _)) = probe(&["describe", "set"]) {
            if !out.contains("用法: set <记录> <字段> <值>") {
                bad.push("describe set 文本里没有用法行".to_string());
            }
        }
        if bad.is_empty() {
            pass += 1;
            msgs.push("PASS  cat 只出正文、describe 给出调用形状".to_string());
        } else {
            fail += 1;
            msgs.push(format!("FAIL  cat/describe: {}", bad.join("; ")));
        }
    }

    // --- the first write into an empty library must map the record ------
    {
        let (ok, msg) = case_first_write_maps_record();
        if ok {
            pass += 1;
            msgs.push(msg);
        } else {
            fail += 1;
            msgs.push(msg);
        }
    }

    // --- a write may only move the bytes the user changed ---------------
    {
        let (ok, msg) = case_minimal_write_back();
        if ok {
            pass += 1;
            msgs.push(msg);
        } else {
            fail += 1;
            msgs.push(format!("FAIL  {msg}"));
        }
    }

    // --- normalize may only rewrite the ID line -------------------------
    // `normalize` upgrades a legacy file name. It writes too, and the first
    // version of that write called `render()`, which re-emits the header in
    // canonical key order. On the shipped library that split every packed
    // `ID=..;PROJECT=..;TASK=..;SESSION=..` line into four, added `KEYS=` and
    // `SIZE=` to 39 records, and dropped keys `render()` does not model -- 228
    // records rewritten when only the file name was supposed to change.
    //
    // The first version of this check called `render_for_write` directly, which
    // was already correct: it passed with the defect fully in place, so it
    // proved nothing. It has to drive the real rename path, because that is
    // where the wrong serialiser was called. A record on disk under a legacy
    // name is the only input that reaches it.
    {
        let root = std::env::temp_dir().join(format!("abook-norm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let recs = root.join("canonical/records");
        let packed = "[CANONICAL_MEMORY]\n\
FORMAT=CANONICAL_MEMORY_V2\n\
ID=AB2-C-001;PROJECT=agentsbook;TASK=migration;SESSION=migration;SEQUENCE=1\n\
KIND=DECISION\n\
TITLE=标题;SUMMARY=摘要，带全角标点；不该被改写\n\
CONCEPTS=甲,乙;ENTITIES=X,Y\n\
STATUS=ACTIVE\n\
[/CANONICAL_MEMORY]\n\
[MEMORY_BODY]\n\
正文一行\n\
[/MEMORY_BODY]\n";
        let ok = std::fs::create_dir_all(&recs).is_ok()
            && std::fs::write(recs.join("AB2-C-001.md"), packed).is_ok();
        if !ok {
            fail += 1;
            msgs.push("FAIL  normalize 用例无法建立样例目录".to_string());
        } else {
            let renamed = crate::cmd_admin::normalize_names(
                &root, crate::rec::Lib::Canonical, false,
            );
            match renamed {
                Err(e) => {
                    fail += 1;
                    msgs.push(format!("FAIL  normalize 执行失败: {e:?}"));
                }
                Ok(ch) if ch.is_empty() => {
                    fail += 1;
                    msgs.push("FAIL  normalize 没有改名这条旧命名记录".to_string());
                }
                Ok(_) => {
                    let after: Vec<_> = std::fs::read_dir(&recs)
                        .map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>())
                        .unwrap_or_default();
                    match after.first().map(|p| std::fs::read_to_string(p)) {
                        Some(Ok(out)) => {
                            // Everything except the ID line must survive byte for byte.
                            let strip = |t: &str| -> String {
                                t.lines()
                                    .filter(|l| !l.starts_with("ID="))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            };
                            if after.len() != 1 {
                                fail += 1;
                                msgs.push(format!(
                                    "FAIL  normalize 留下了 {} 个文件，应为 1",
                                    after.len()
                                ));
                            } else if strip(&out) != strip(packed) {
                                fail += 1;
                                msgs.push(format!(
                                    "FAIL  改名波及了 ID 以外的字节\n  期望={:?}\n  实得={:?}",
                                    strip(packed),
                                    strip(&out)
                                ));
                            } else if !out.contains(";PROJECT=agentsbook;TASK=migration;SESSION=migration;SEQUENCE=1") {
                                fail += 1;
                                msgs.push(format!("FAIL  改名把打包行拆开了:\n{out}"));
                            } else {
                                pass += 1;
                                msgs.push("归一化只改 ID 行，其余字节不动：通过".to_string());
                            }
                        }
                        _ => {
                            fail += 1;
                            msgs.push("FAIL  normalize 后读不到记录".to_string());
                        }
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- Atlas: a bare PATH must resolve under <lib>/records -------------
    // `Lib::dir()` already ends in `/records`; joining twice produced
    // `<root>/canonical/records/records/x.md`, so every size lookup failed and
    // MAXSIZE was rewritten to 0 on the card with the most records.
    {
        let root = std::path::Path::new("/r");
        let full = crate::atlas::resolve_path(root, crate::rec::Lib::Canonical, "canonical/records/a.md");
        let bare = crate::atlas::resolve_path(root, crate::rec::Lib::Canonical, "a.md");
        let want = std::path::PathBuf::from("/r/canonical/records/a.md");
        if bare != want {
            fail += 1;
            msgs.push(format!("FAIL  裸名解析成 {bare:?}，应为 {want:?}"));
        } else if full != want {
            fail += 1;
            msgs.push(format!("FAIL  全路径解析成 {full:?}，应为 {want:?}"));
        } else {
            pass += 1;
            msgs.push("PASS  PATH 裸名与全路径都解析到 records 目录".into());
        }
    }

    // Semantic search must be able to reach an identifier by its component
    // words. Measured on the real library: `search GATE --level 3` returned 1
    // record while a literal scan found 169, because `tokenize` swallowed
    // `PLAN_ACCEPTANCE_DOC_GATE` whole. This library's vocabulary is dominated
    // by such identifiers, so the failure mode is severe and silent.
    {
        let toks = crate::semantic::tokenize("PLAN_ACCEPTANCE_DOC_GATE");
        let has_gate = toks.iter().any(|t| t == "gate");
        let has_whole = toks
            .iter()
            .any(|t| t == "plan_acceptance_doc_gate");
        if !has_gate {
            fail += 1;
            msgs.push("FAIL  分词未把标识符切成子词，`GATE` 无法命中 KEYS".into());
        } else if !has_whole {
            fail += 1;
            msgs.push("FAIL  切子词时丢了整词，完整拼写反而搜不到".into());
        } else {
            pass += 1;
            msgs.push("PASS  标识符既保留整词又可被 `GATE` 这类子词命中".into());
        }
    }

    // Every command that advertises `--json` must emit exactly one parseable
    // JSON document. This case exists because 11 commands accepted the flag and
    // ignored it: `list --json` returned 0 and printed a table, so a caller
    // reading JSON got a parse error or, worse, silently parsed nothing.
    {
        // Drive the real binary rather than an internal call: the defect lived
        // in what actually reached stdout, which an in-process probe cannot see.
        let exe = self_path();
        let live = crate::util::resolve_root(&crate::util::Args::default());
        // Run against a COPY, never the real library: these samples include
        // write commands (`commit --slug t --body x` among them), and running
        // them in place silently appended junk commits to real history.
        let sbx = sandbox_copy(&live);
        // Never let a degenerate sandbox point the writes back at the library.
        // The helper is deliberately simple so it can be broken on purpose to
        // test this check, and a broken build returning its source is exactly
        // the shape of that failure. Redirect to an empty throwaway dir and
        // record the defect instead of writing anywhere real.
        let mut sbx_degenerate = false;
        let root = if sbx.path == live {
            sbx_degenerate = true;
            let d = std::env::temp_dir().join(format!("abook-degenerate-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&d);
            d
        } else {
            sbx.path.clone()
        };
        let before = tree_fingerprint(&live);
        // Derived from the spec table, not hand-written: the hard-coded list
        // had 13 entries while 39 commands advertise the flag, so 26 promises
        // were never exercised. Each sample is a real invocation that works
        // against the shipped library.
        let mut cmds: Vec<Vec<String>> = Vec::new();
        for (name, _params, _flags, _sum, sample) in crate::spec::SPEC.iter() {
            if !crate::spec::takes_json(name) {
                continue;
            }
            let mut v: Vec<String> = vec![name.to_string()];
            for tok in sample.split_whitespace() {
                if tok.starts_with('@') {
                    // Placeholder: filled from the library's own first record.
                    v.push(tok.to_string());
                } else {
                    v.push(tok.to_string());
                }
            }
            cmds.push(v);
        }
        // `works` filters out the samples that cannot run on this machine
        // (write commands needing --approve, or an argument this build has no
        // fixture for). A sample is only kept if it succeeds without --json,
        // which proves the shape test below measures the flag and not the
        // command's own brokenness.
        let mut bad: Vec<String> = Vec::new();
        match exe {
            None => bad.push("无法定位自身可执行文件".to_string()),
            Some(ref e) if !e.is_absolute() => {
                bad.push(format!("current_exe 非绝对路径: {}", e.display()));
                let _ = e;
            }
            Some(exe) => {
                let first = first_record_arg(&root);
                let mut skipped: Vec<String> = Vec::new();
                for c in &cmds {
                    let resolved: Vec<String> = c
                        .iter()
                        .map(|t| if t == "@any" { first.clone() } else { t.clone() })
                        .collect();
                    let run = |extra: &[&str]| {
                        let mut cc = std::process::Command::new(&exe);
                        cc.args(&resolved);
                        for e in extra {
                            cc.arg(e);
                        }
                        cc.arg("--root").arg(&root).output()
                    };
                    // Baseline first: a sample that fails on its own has
                    // nothing to say about --json.
                    match run(&[]) {
                        Ok(b) if b.status.code() != Some(0) => {
                            skipped.push(format!("abook {}", resolved.join(" ")));
                            continue;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            bad.push(format!("abook {} 无法执行: {e}", resolved.join(" ")));
                            continue;
                        }
                    }
                    match run(&["--json"]) {
                        Ok(o) => {
                            let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            if t.is_empty() {
                                bad.push(format!(
                                    "abook {} --json 无输出 (stderr: {})",
                                    resolved.join(" "),
                                    crate::util::truncate_chars(
                                        String::from_utf8_lossy(&o.stderr).trim(), 60
                                    )
                                ));
                            } else if !json_shape_ok(&t) {
                                bad.push(format!(
                                    "abook {} --json -> {}",
                                    resolved.join(" "),
                                    crate::util::truncate_chars(&t, 36).replace('\n', " ")
                                ));
                            }
                        }
                        Err(e) => bad.push(format!("abook {} 无法执行: {e}", resolved.join(" "))),
                    }
                }
                if !skipped.is_empty() {
                    msgs.push(format!(
                        "      (跳过 {} 条本机跑不了的样例: {})",
                        skipped.len(),
                        skipped.join("; ")
                    ));
                }
            }
        }
        // The copy absorbed every write, so the live tree must be byte-for-byte
        // the same size it was. This is the assertion that would have caught
        // the junk commits on day one instead of commit 127.
        let after = tree_fingerprint(&live);
        if sbx_degenerate {
            bad.push("沙箱退化成了真库（写入已被改道到临时目录）".to_string());
        }
        if before != after {
            bad.push(format!(
                "真库被写入: {} 个文件/{} 字节 → {} 个文件/{} 字节",
                before.0, before.1, after.0, after.1
            ));
        }
        if bad.is_empty() {
            pass += 1;
            msgs.push(format!("PASS  {} 条声明 --json 的命令都输出单个可解析 JSON 文档", cmds.len()));
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  {} 条命令的 --json 不是可解析 JSON: {}",
                bad.len(),
                bad.join("; ")
            ));
        }
    }

    // ---- a no-op edit must not pretend to have changed anything
    //
    // `save` wrote the file, rewrote the map, bumped the index and appended a
    // change record for a value that was already there. Nothing complained,
    // because every individual step is legal; the lie is only visible across
    // the whole set. A bulk "normalise" pass over the library added 286
    // phantom commits in a single test run before this was caught.
    if let Some(exe) = self_path() {
        let live = crate::util::resolve_root(&crate::util::Args::default());
        let sbx = sandbox_copy(&live);
        let root = sbx.path.clone();
        if root == live {
            fail += 1;
            msgs.push("FAIL  no-op 探针的沙箱退化成了真库，已跳过".into());
        } else {
            let first = first_candidate_arg(&root);
            let run = |args: &[&str]| -> (String, i32) {
                match std::process::Command::new(&exe)
                    .args(args)
                    .arg("--root")
                    .arg(&root)
                    .output()
                {
                    Ok(o) => (
                        String::from_utf8_lossy(&o.stdout).to_string(),
                        o.status.code().unwrap_or(-1),
                    ),
                    Err(_) => (String::new(), -1),
                }
            };
            let count_f = |d: &str| -> usize {
                std::fs::read_dir(root.join(d))
                    .map(|r| r.flatten().count())
                    .unwrap_or(0)
            };
            let (title, _) = run(&["get", &first, "--field", "TITLE"]);
            let title = title.trim().to_string();
            let commits_before = count_f("commits");
            let (out, rc) = run(&["set", &first, "TITLE", "--value", &title]);
            let commits_after = count_f("commits");
            // Both signs must be present: the journal must not grow, and the
            // caller must be told nothing was written. Reporting success
            // silently would leave a script believing it changed something.
            let quiet = commits_before == commits_after;
            let honest = out.contains("未写入");
            // A real change must still be journalled, or this "fix" would have
            // disabled the change log outright.
            let (_, _) = run(&["set", &first, "TITLE", "--value", "自检改的标题"]);
            let commits_moved = count_f("commits") == commits_after + 1;
            let _ = rc;
            if quiet && honest && commits_moved {
                pass += 1;
                msgs.push("PASS  同值写入不写盘、不记变更、并如实告知；真改动仍然记变更".into());
            } else {
                fail += 1;
                msgs.push(format!(
                    "FAIL  同值写入: 变更记录不变={quiet} 告知未写入={honest} 真改动记变更={commits_moved}"
                ));
            }
        }
    }

    // ---- staging / review must be readable, paged and honest about subdirs
    //
    // These directories existed in the live library (1 staging item, 20 review
    // files plus `abandoned-20260928/`) while `paths` advertised them and no
    // command could open them. A listing that silently skips a subdirectory is
    // worse than no listing, so all three behaviours are pinned here.
    if let Some(exe) = self_path() {
        let root = crate::util::resolve_root(&crate::util::Args::default());
        let run = |args: &[&str]| -> (String, String, i32) {
            match std::process::Command::new(&exe)
                .args(args)
                .arg("--root")
                .arg(&root)
                .output()
            {
                Ok(o) => (
                    String::from_utf8_lossy(&o.stdout).to_string(),
                    String::from_utf8_lossy(&o.stderr).to_string(),
                    o.status.code().unwrap_or(-1),
                ),
                Err(e) => (String::new(), format!("{e}"), -1),
            }
        };

        // (a) paging is honoured rather than fixed at some hidden default
        //
        // Run against a throwaway root with three review files, not the live
        // library. The old version asserted `p1 != p1b` against whatever the
        // real `review/` held; with a single file `Page::from_args` clamps
        // `--page 2` back to page 1, so the two outputs are necessarily equal
        // and the check could never pass on a healthy small library. A fixture
        // makes the assertion test paging instead of the data.
        let pdir = std::env::temp_dir().join(format!(
            "abook-page-{}-{}",
            std::process::id(),
            crate::util::now_secs()
        ));
        let _ = std::fs::remove_dir_all(&pdir);
        let mut fixture_ok = std::fs::create_dir_all(pdir.join("review")).is_ok();
        if fixture_ok {
            for n in 1..=3 {
                if std::fs::write(
                    pdir.join(format!("review/2026010{n}-page-{n}.md")),
                    format!("# page {n}\n\nbody {n}\n"),
                )
                .is_err()
                {
                    fixture_ok = false;
                }
            }
        }
        let ppr = |args: &[&str]| -> String {
            match std::process::Command::new(&exe)
                .args(args)
                .arg("--root")
                .arg(&pdir)
                .output()
            {
                Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                Err(_) => String::new(),
            }
        };
        let (p1, p1b, pa) = if fixture_ok {
            (
                ppr(&["review", "--per-page", "1"]),
                ppr(&["review", "--per-page", "1", "--page", "2"]),
                ppr(&["review", "--limit", "1"]),
            )
        } else {
            (String::new(), String::new(), String::new())
        };
        let counted = |t: &str| -> usize { t.lines().filter(|l| l.contains(".md")).count() };
        let _ = std::fs::remove_dir_all(&pdir);
        let page_ok =
            counted(&p1) == 1 && counted(&p1b) == 1 && counted(&pa) == 1 && p1 != p1b;
        if !fixture_ok {
            skip += 1;
            msgs.push("SKIP  建不出分页夹具目录，跳过分页检查".into());
        } else if page_ok {
            pass += 1;
            msgs.push("PASS  staging/review 分页生效，--limit 与 --page 各自独立".into());
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  review 分页失效: per-page1 {} 行 / page2 {} 行 / limit1 {} 行",
                counted(&p1),
                counted(&p1b),
                counted(&pa)
            ));
        }

        // (b) a subdirectory is reported, not dropped
        let (all, _, _) = run(&["review", "--per-page", "0"]);
        let disk_dirs = std::fs::read_dir(root.join("review"))
            .map(|r| r.flatten().filter(|e| e.path().is_dir()).count())
            .unwrap_or(0);
        let mentions = all.contains("子目录");
        if (disk_dirs == 0 && !mentions) || (disk_dirs > 0 && mentions) {
            pass += 1;
            msgs.push(format!(
                "PASS  目录列表不静默跳过子目录（review 下有 {disk_dirs} 个）"
            ));
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  review 下有 {disk_dirs} 个子目录，列表却没有报出来"
            ));
        }

        // (c) --show opens a real file, and a missing name is an error not a blank
        let any = std::fs::read_dir(root.join("review"))
            .ok()
            .and_then(|r| {
                r.flatten()
                    .find(|e| e.path().is_file())
                    .map(|e| e.file_name().to_string_lossy().to_string())
            });
        match any {
            None => {
                skip += 1;
                msgs.push("SKIP  review 为空，跳过 --show 检查".into());
            }
            Some(name) => {
                let (body, _, rc) = run(&["review", "--show", &name]);
                let (_, _, rc_missing) = run(&["review", "--show", "确定不存在的文件.md"]);
                if rc == 0 && body.trim().len() > 10 && rc_missing == 2 {
                    pass += 1;
                    msgs.push("PASS  --show 能读全文，写错文件名返回退出码 2".into());
                } else {
                    fail += 1;
                    msgs.push(format!(
                        "FAIL  --show 正常rc={rc} 正文{}字节，缺文件rc={rc_missing}（应分别为 0 与 2）",
                        body.trim().len()
                    ));
                }
            }
        }
    } else {
        skip += 3;
        msgs.push("SKIP  无稳定自身路径，跳过 staging/review 检查".into());
    }

    // `abook version` carries the source fingerprint that `build.rs` embedded.
    // `tools/preflight.sh` compares it against the working tree to prove the
    // binary on disk was built from the sources on disk -- the check that mtime
    // based comparisons kept getting wrong in both directions. If the
    // fingerprint ever stops being reported, that preflight step degrades into
    // a silent skip, so guard the contract here.
    {
        let fp = crate::SRC_FP;
        let looks_like_hash =
            fp.len() == 16 && fp.chars().all(|c| c.is_ascii_hexdigit()) && fp != "0000000000000000";
        if looks_like_hash {
            pass += 1;
            if verbose {
                msgs.push(format!("PASS  version 报告源码指纹 {fp}（预检据此判断二进制是否过期）"));
            }
        } else {
            fail += 1;
            msgs.push(format!(
                "FAIL  version 的源码指纹异常 {fp:?}（应 16 位十六进制且非全零；预检会因此静默跳过）"
            ));
        }
    }

    // The first thing anyone types. Both used to fail in ways that read like
    // the tool itself is broken: `--version` demanded a value and `--help`
    // reported "需要子命令". They are also the two flags a caller is most
    // likely to combine with a global option, so exercise that too.
    if let Some(exe) = self_path() {
        let go = |args: &[&str]| -> (String, i32) {
            match std::process::Command::new(&exe).args(args).output() {
                Ok(o) => (
                    String::from_utf8_lossy(&o.stdout).to_string()
                        + &String::from_utf8_lossy(&o.stderr),
                    o.status.code().unwrap_or(-1),
                ),
                Err(e) => (format!("{e}"), -1),
            }
        };
        let mut bad: Vec<String> = Vec::new();
        for argv in [["--version"], ["-V"], ["--help"], ["-h"]] {
            let (text, rc) = go(&argv);
            let t = text.trim();
            if rc != 0 {
                bad.push(format!("{argv:?} 退出码 {rc}"));
            } else if t.contains("需要一个值") || t.contains("需要子命令") {
                bad.push(format!("{argv:?} 被当成缺值或缺少子命令"));
            }
        }
        let (v2, rc2) = go(&["--root", ".", "--version"]);
        if rc2 != 0 || !v2.contains("abook ") {
            bad.push(format!("`--root . --version` 失效 rc={rc2}"));
        }
        if bad.is_empty() {
            pass += 1;
            if verbose {
                msgs.push("PASS  --version/-V/--help/-h 四种写法都能直接生效".into());
            }
        } else {
            fail += 1;
            msgs.push(format!("FAIL  首问参数 {} 项不通：{}", bad.len(), bad.join("；")));
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过首问参数检查".into());
    }

    // A record living under several branches is normal -- the map is a branch
    // view, not a directory listing. `reconcile` used to count those as
    // duplicates and declare an inconsistent library, which is the worst kind
    // of false alarm: it invites "repairing" data that was never broken. The
    // two cases must stay separated, so both are pinned here.
    if let Some(exe) = self_path() {
        let tmp = std::env::temp_dir().join(format!("abook-rc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // The layout reconcile reads is <root>/canonical/{records,Atlas.md},
        // not a flat directory. Building it flat made every subprocess exit 6
        // ("读取失败"), which the assertions then reported as wrong behaviour
        // -- the test was broken, not the code.
        let mk = |sub: &str, atlas: &str| -> std::path::PathBuf {
            let root = tmp.join(sub);
            let lib = root.join("canonical");
            let _ = std::fs::create_dir_all(lib.join("records"));
            let _ = std::fs::create_dir_all(lib.join("commits"));
            let _ = std::fs::create_dir_all(root.join("candidate/records"));
            let _ = std::fs::write(lib.join("records/AB2-X-001.md"), "ID=AB2-X-001\nTITLE=甲\n");
            let _ = std::fs::write(lib.join("records/AB2-X-002.md"), "ID=AB2-X-002\nTITLE=乙\n");
            let _ = std::fs::write(lib.join("Atlas.md"), atlas);
            let _ = std::fs::write(
                root.join("candidate/Atlas.md"),
                "# Atlas\n",
            );
            root
        };
        // 同一条记录挂两个分支：合法，不能算问题。
        let multi = mk(
            "multi",
            "# Atlas\n\n## 甲支\nBRANCH=甲支\nPATH=canonical/records/AB2-X-001.md,AB2-X-002.md\n\n## 乙支\nBRANCH=乙支\nPATH=canonical/records/AB2-X-001.md\n",
        );
        // 同一分支内出现两次：真问题，其中一条永远不可达。
        let dupe = mk(
            "dupe",
            "# Atlas\n\n## 甲支\nBRANCH=甲支\nPATH=canonical/records/AB2-X-001.md,AB2-X-001.md,AB2-X-002.md\n",
        );
        let go = |root: &std::path::Path| -> (String, i32) {
            match std::process::Command::new(&exe)
                .args(["reconcile", "--json", "--root"])
                .arg(root)
                .output()
            {
                Ok(o) => (
                    String::from_utf8_lossy(&o.stdout).to_string(),
                    o.status.code().unwrap_or(-1),
                ),
                Err(e) => (format!("{e}"), -1),
            }
        };
        let (mo, mrc) = go(&multi);
        let (do_, drc) = go(&dupe);
        let mut bad: Vec<String> = Vec::new();
        // multi: canonical 段必须 真重复=0 且 consistent=true
        let seg = |body: &str, key: &str| -> String {
            body.split(key)
                .nth(1)
                .and_then(|t| t.split('}').next())
                .unwrap_or("")
                .to_string()
        };
        let mc = seg(&mo, "\"lib\":\"canonical\"");
        if !mc.contains("\"duplicates\":0") {
            bad.push(format!("多分支归属被误报为重复 rc={mrc}"));
        }
        if !mc.contains("\"consistent\":true") {
            bad.push("多分支归属被判成不一致（真库会因此报 rc=5）".into());
        }
        if !mc.contains("\"multi_branch\":1") {
            bad.push("多分支归属条数没单独报出来".into());
        }
        // dupe: 必须报出 1 条真重复且不一致
        let dc = seg(&do_, "\"lib\":\"canonical\"");
        if !dc.contains("\"duplicates\":1") || !dc.contains("AB2-X-001") {
            bad.push(format!("同一分支内的真重复没被抓到 rc={drc}"));
        }
        if !dc.contains("\"consistent\":false") {
            bad.push("真重复被判成一致".into());
        }
        let _ = std::fs::remove_dir_all(&tmp);
        if bad.is_empty() {
            pass += 1;
            if verbose {
                msgs.push("PASS  reconcile 区分多分支归属与同分支真重复".into());
            }
        } else {
            fail += 1;
            msgs.push(format!("FAIL  多分支/真重复判别错误：{}", bad.join("；")));
        }
    } else {
        skip += 1;
        msgs.push("SKIP  无稳定自身路径，跳过 reconcile 判别检查".into());
    }

    (pass, fail, skip, msgs)
}

fn ok_workflow(script: &std::path::Path, root: &std::path::Path) -> bool {
    std::process::Command::new("python3")
        .arg(script)
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ok_docs(script: &std::path::Path, root: &std::path::Path) -> bool {
    std::process::Command::new("python3")
        .arg(script)
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ok_preflight(script: &std::path::Path) -> bool {
    std::process::Command::new("sh")
        .arg(script)
        .arg("--verify-exit")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Locate this binary without trusting `current_exe()`.
///
/// On Android the kernel execs `/apex/com.android.runtime/bin/linker64`, which
/// then loads the application, so `/proc/self/exe` resolves to the linker. Any
/// child spawned from it fails with `expected absolute path`. The real path is
/// the first field of `/proc/self/cmdline`; fall back to skipping the test if
/// even that is not an existing file.
fn self_path() -> Option<std::path::PathBuf> {
    if let Ok(raw) = std::fs::read("/proc/self/cmdline") {
        if let Some(end) = raw.iter().position(|&b| b == 0) {
            let s = String::from_utf8_lossy(&raw[..end]).to_string();
            if !s.is_empty() {
                let p = std::path::PathBuf::from(&s);
                let abs = if p.is_absolute() {
                    p
                } else if p.components().count() > 1 {
                    // `./abook` or `tools/abook`: relative to the working
                    // directory, never on PATH. Resolving via PATH here made
                    // `--json` look unsupported when the binary was run as
                    // `./ab.new`.
                    std::env::current_dir().ok()?.join(p)
                } else {
                    // Resolve through PATH ourselves; execvp would too, but we
                    // need a concrete path to check existence first.
                    let mut found = None;
                    for d in crate::util::path_dirs() {
                        let cand = std::path::Path::new(&d).join(&s);
                        if cand.is_file() {
                            found = Some(cand);
                            break;
                        }
                    }
                    found?
                };
                if abs.is_file() {
                    return Some(abs);
                }
            }
        }
    }
    let ce = std::env::current_exe().ok()?;
    if ce.is_file() {
        Some(ce)
    } else {
        None
    }
}

/// Structural check that does not need a JSON crate: balanced brackets outside
/// strings, and a first non-space byte of `{` or `[`. Catches concatenated
/// objects (JSONL where JSON was promised) and truncated output.
fn json_shape_ok(t: &str) -> bool {
    // Every line must stand alone as JSON.
    //
    // The whole-output check below accepted a document plus a trailing prose
    // footer, because it only asserted that the first byte was `{` and that the
    // brackets balanced -- and `{"a":1}` followed by `第 1/17 页 共 509 条`
    // satisfies both. That footer really was arriving in the export stream, and
    // this probe called it clean. Line granularity is what a JSONL reader
    // actually consumes, so that is what gets asserted: NDJSON passes, one
    // document passes, anything with a stray sentence fails.
    for line in t.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if !json_one_ok(l) {
            return false;
        }
    }
    true
}

/// One standalone JSON value.
fn json_one_ok(t: &str) -> bool {
    let b: Vec<char> = t.chars().collect();
    // A bare text line has balanced (empty) brackets and no open string, so
    // balance alone cannot tell JSON from prose. Anchor on the first byte.
    if !matches!(b.first(), Some('{') | Some('[')) {
        return false;
    }
    let mut stack: Vec<char> = Vec::new();
    let mut in_str = false;
    let mut esc = false;
    for &c in &b {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => stack.push(c),
            '}' => {
                if stack.pop() != Some('{') {
                    return false;
                }
            }
            ']' => {
                if stack.pop() != Some('[') {
                    return false;
                }
            }
            _ => {}
        }
    }
    !in_str && stack.is_empty()
}

/// A write may only move the bytes the user actually changed.
///
/// The first version of the editor re-rendered the whole record to write one
/// field back. This library packs several fields on one line
/// (`ID=..;PROJECT=..;TASK=..;SESSION=..`), so setting a field to the value it
/// already had quietly expanded one line into four and rewrote full-width
/// punctuation elsewhere -- 92 of 189 candidate records changed on a no-op
/// write. Nothing downstream noticed, because every reader still saw the right
/// values. Byte identity is therefore the only assertion that catches it.
/// Writing into a brand-new library must produce an atlas that actually maps
/// the record.
///
/// `Atlas::load` falls back to treating the whole file as `head_raw` when it
/// finds no cards. `render` then emitted only each card's raw bytes, and a
/// card created during the run has none -- so the first write produced an
/// atlas with `RECORD_COUNT=1` and zero `PATH` lines. Nothing failed: the
/// record was on disk and readable by name, it just could not be found through
/// the map. Only a genuinely empty library hits this, which is why testing on
/// a copy of the real one never showed it, and why every new user would.
fn case_first_write_maps_record() -> (bool, String) {
    let dir = std::env::temp_dir().join(format!(
        "abook-first-{}-{}",
        std::process::id(),
        crate::util::now_secs()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return (false, "FAIL  空库首写：建临时目录失败".into());
    }
    let self_exe = match self_path() {
        Some(p) => p,
        None => return (false, "FAIL  空库首写：取不到自身路径".into()),
    };
    let out = std::process::Command::new(&self_exe)
        .args([
            "write",
            "--root",
            dir.to_str().unwrap_or("."),
            "--title",
            "空库首写",
            "--read-when",
            "需要时",
            "--not-for",
            "不需要时",
            "--body",
            "正文",
        ])
        .output();
    let ok_run = matches!(&out, Ok(o) if o.status.success());

    let atlas = std::fs::read_to_string(dir.join("candidate/Atlas.md")).unwrap_or_default();
    let paths = atlas.lines().filter(|l| l.starts_with("PATH=")).count();
    let recs = std::fs::read_dir(dir.join("candidate/records"))
        .map(|d| d.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    let _ = std::fs::remove_dir_all(&dir);

    if !ok_run {
        return (false, "FAIL  空库首写：write 命令执行失败".into());
    }
    if recs != 1 {
        return (
            false,
            format!("FAIL  空库首写：应写入 1 条记录，实际 {recs} 条"),
        );
    }
    if paths != 1 {
        return (
            false,
            format!(
                "FAIL  空库首写：记录写了但地图没有 PATH（应 1 条，实际 {paths} 条）\
                 ——记录存在却无法通过地图找到"
            ),
        );
    }
    (
        true,
        "PASS  空库第一次写入，记录同时进入地图（PATH 1 条）".into(),
    )
}

fn case_minimal_write_back() -> (bool, String) {
    let packed = "[CANDIDATE_MEMORY]
FORMAT=CANDIDATE_MEMORY_V2
ID=N-20260101T000000-aaaaaaaa;PROJECT=p;TASK=t;SESSION=s
KIND=DECISION
TITLE=标题
SUMMARY=摘要，带全角标点；不该被改写
[/CANDIDATE_MEMORY]
[MEMORY_BODY]
正文一行
[/MEMORY_BODY]
";
    let path = std::path::PathBuf::from("N-20260101T000000-aaaaaaaa.md");
    let mut rec = match crate::rec::Record::parse_str(packed, &path, crate::rec::Lib::Candidate) {
        Ok(r) => r,
        Err(e) => return (false, format!("样例解析失败: {e:?}")),
    };

    // 1. Setting a field to the value it already holds must not move a byte.
    rec.set("TITLE", "标题");
    let same = rec.render_for_write();
    if same != packed {
        return (
            false,
            format!("同值写回改动了字节\n  in ={packed:?}\n  out={same:?}"),
        );
    }

    // 2. Changing one field inside a packed line must leave the line packed and
    //    the other three fields untouched.
    //
    //    The field chosen here is deliberately the *middle* one. Editing the
    //    last field cannot expose an off-by-one in the value span, because the
    //    line ends right after it: a span that reaches too far and a span that
    //    stops short differ only when something follows. `PROJECT` is followed
    //    by two more fields, so any span error corrupts them visibly.
    rec.set("PROJECT", "NEW");
    let one = rec.render_for_write();
    let want_packed =
        "ID=N-20260101T000000-aaaaaaaa;PROJECT=NEW;TASK=t;SESSION=s".to_string();
    if !one.contains(&want_packed) {
        return (false, format!("改字段后打包行被拆开:\n{one}"));
    }
    // 3. The rest of the file, down to the last byte, must be identical.
    let stripped = one.replace("PROJECT=NEW", "PROJECT=p");
    if stripped != packed {
        return (
            false,
            format!("改字段波及了无关字节\n  期望={packed:?}\n  实得={stripped:?}"),
        );
    }

    // 4. A new field gets its own line; nothing existing is disturbed.
    let mut rec2 = crate::rec::Record::parse_str(packed, &path, crate::rec::Lib::Candidate)
        .expect("样例解析失败");
    rec2.set("EVIDENCE", "证据");
    let added = rec2.render_for_write();
    if !added.contains("\nEVIDENCE=证据\n") || !added.contains(&want_packed[..0]) {
        return (false, format!("新增字段未独占一行:\n{added}"));
    }
    let without_new = added.replace("EVIDENCE=证据\n", "");
    if without_new != packed {
        return (false, format!("新增字段波及无关字节:\n{added}"));
    }

    // 5. Dropping a field from a packed line keeps the others on that line.
    let mut rec3 = crate::rec::Record::parse_str(packed, &path, crate::rec::Lib::Candidate)
        .expect("样例解析失败");
    rec3.fields.retain(|(k, _)| k != "SESSION");
    let dropped = rec3.render_for_write();
    if !dropped.contains("ID=N-20260101T000000-aaaaaaaa;PROJECT=p;TASK=t\n") {
        return (false, format!("删字段后残留分隔符或拆行:\n{dropped}"));
    }

    // 6. Body edits touch only the body.
    let mut rec4 = crate::rec::Record::parse_str(packed, &path, crate::rec::Lib::Candidate)
        .expect("样例解析失败");
    // Body values are stored without surrounding newlines, matching what the
    // parser yields, so extend in the middle rather than appending a trailing
    // one.
    rec4.body = format!("{}\n追加一行", rec4.body);
    let with_body = rec4.render_for_write();
    if !with_body.contains("正文一行\n追加一行\n") {
        return (false, format!("正文追加未落在正文里:\n{with_body}"));
    }
    let body_undo = with_body.replace("追加一行\n", "");
    if body_undo != packed {
        return (false, format!("正文追加波及头部:\n{with_body}"));
    }

    // 7. A line whose value carries trailing spaces must still be spliced
    //    precisely: rewriting the last field may not eat the padding.
    let padded = "[CANDIDATE_MEMORY]\nFORMAT=CANDIDATE_MEMORY_V2\nID=N-20260101T000000-bbbbbbbb;TITLE=标题;SIZE=SMALL   \nKIND=DECISION\n[/CANDIDATE_MEMORY]\n[MEMORY_BODY]\n正文\n[/MEMORY_BODY]\n";
    if let Ok(mut rec5) =
        crate::rec::Record::parse_str(padded, &path, crate::rec::Lib::Candidate)
    {
        rec5.set("ID", "N-20260101T000000-cccccccc");
        let out5 = rec5.render_for_write();
        if !out5.contains("ID=N-20260101T000000-cccccccc;TITLE=标题;SIZE=SMALL") {
            return (false, format!("改首字段波及同行后续内容:\n{out5}"));
        }
    }

    (
        true,
        "PASS  写回只改目标区间：同值零改动、打包行不拆、改中间字段不波及后续、新增不扰他行、删字段留同行、正文追加只动正文"
            .into(),
    )
}

/// Expose the commit writer's pre-write check to the self-test.
fn vet_commit_for_test(name: &str, block: &str) -> Result<(), String> {
    crate::cmd_admin::vet_commit_public(name, block)
}

pub fn cmd_selftest(a: &crate::util::Args) -> crate::util::R<i32> {
    // `--only N` 只跑第 N 条用例。
    //
    // 这个参数以前只在帮助示例里出现过，参数表里没有，于是 `selftest --only 1`
    // 直接被"不接受 --only"拒掉——**帮助里宣传的参数必须真的能用**，否则
    // 文档就是错的。示例文字也是契约的一部分。
    let only = a.num("only").map(|n| n as usize);
    let (pass, fail, skip, msgs) = run(a.has("verbose") || a.has("all"), only);
    // `--json` reports the tally, not the transcript. The per-case lines stay on
    // stderr because they are the human view; a caller that asked for JSON is
    // deciding pass/fail on the counts, and wrapping 145 prose lines into an
    // array would bury the two numbers that matter.
    let want_json = a.has("json");
    for m in &msgs {
        if want_json && !m.starts_with("FAIL") {
            continue;
        }
        println!("{m}");
    }
    if want_json {
        println!(
            "{{\"pass\":{pass},\"fail\":{fail},\"skip\":{skip},\"ok\":{}}}",
            fail == 0
        );
        return Ok(if fail == 0 { 0 } else { 5 });
    }
    if skip > 0 {
        println!("自检 {pass} 通过 / {fail} 失败 / {skip} 跳过（仅仓库内可跑的检查）");
    } else {
        println!("自检 {pass} 通过 / {fail} 失败");
    }
    if fail == 0 {
        Ok(0)
    } else {
        Ok(5)
    }
}

/// The first record in the library, used to fill `@any` in spec samples.
/// A throwaway copy of the library for probes that may write.
///
/// This exists because they used to write to the real one. The `--json`
/// coverage probe runs every documented sample verbatim, and `commit`'s sample
/// is `--slug t --body x`; running the self-test therefore appended a
/// three-field junk commit to the live library every single time. Twenty of
/// them accumulated before anyone noticed, and they were indistinguishable
/// from real history to a reader.
///
/// A probe must never be able to do that. Copying first costs a few hundred
/// milliseconds and makes the whole class impossible.
struct Sandbox {
    path: std::path::PathBuf,
    /// Only a path this process created under the temp dir may ever be
    /// deleted. Without this flag, a sandbox helper that degenerates into
    /// returning its source (which is exactly what a deliberately broken build
    /// does, and a broken build is what these probes exist to simulate) would
    /// hand the real library to the cleanup step. That is not hypothetical: it
    /// happened, and it deleted the live library outright. An assertion that
    /// the path is disposable has to be structural, not assumed.
    owned: bool,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn sandbox_copy(src: &std::path::Path) -> Sandbox {
    let dst = std::env::temp_dir().join(format!(
        "abook-sbx-{}-{}",
        std::process::id(),
        crate::util::now_secs()
    ));
    let _ = std::fs::remove_dir_all(&dst);
    let mut stack = vec![(src.to_path_buf(), dst.clone())];
    while let Some((from, to)) = stack.pop() {
        if std::fs::create_dir_all(&to).is_err() {
            continue;
        }
        let rd = match std::fs::read_dir(&from) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let fp = e.path();
            let tp = to.join(e.file_name());
            match e.file_type() {
                // Follow symlinks into files, but never into directories: a
                // link loop would make this copy run forever.
                Ok(ft) if ft.is_dir() => stack.push((fp, tp)),
                Ok(ft) if ft.is_symlink() => {
                    if let Ok(m) = std::fs::metadata(&fp) {
                        if m.is_file() {
                            let _ = std::fs::copy(&fp, &tp);
                        }
                    }
                }
                Ok(_) => {
                    let _ = std::fs::copy(&fp, &tp);
                }
                Err(_) => {}
            }
        }
    }
    Sandbox { path: dst, owned: true }
}

/// Cheap fingerprint of a tree: file count plus total bytes. Enough to notice
/// that a probe wrote something it should not have.
fn tree_fingerprint(root: &std::path::Path) -> (usize, u64) {
    let mut n = 0usize;
    let mut b = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(e.path()),
                Ok(_) => {
                    n += 1;
                    b += std::fs::metadata(e.path()).map(|m| m.len()).unwrap_or(0);
                }
                Err(_) => {}
            }
        }
    }
    (n, b)
}

/// A candidate record, not a canonical one.
///
/// Canonical writes require `--approve`, so any probe that writes has to aim at
/// the candidate library or it never reaches the code under test. Picking
/// "the first record" silently selected a canonical one and the probe became a
/// permissions test instead of a no-op test.
fn first_candidate_arg(root: &std::path::Path) -> String {
    if let Some(p) = crate::util::list_md(&root.join(crate::rec::Lib::Candidate.dir()))
        .into_iter()
        .next()
    {
        if let Some(n) = p.file_name() {
            return n.to_string_lossy().trim_end_matches(".md").to_string();
        }
    }
    "none".to_string()
}

fn first_record_arg(root: &std::path::Path) -> String {
    for lib in [crate::rec::Lib::Canonical, crate::rec::Lib::Candidate] {
        if let Some(p) = crate::util::list_md(&root.join(lib.dir())).into_iter().next() {
            if let Some(n) = p.file_name() {
                return n.to_string_lossy().trim_end_matches(".md").to_string();
            }
        }
    }
    "none".to_string()
}
