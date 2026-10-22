//! Built-in self test: adversarial format cases the parser must survive.
//!
//! These are not unit tests of convenience — every case below is drawn from a
//! real deviation measured in the 507-record library, so a regression here is
//! a regression users would feel.

use crate::parse::{parse, Sev};
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

pub fn run(verbose: bool) -> (usize, usize, Vec<String>) {
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut msgs = Vec::new();
    for c in CASES {
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

    (pass, fail, msgs)
}

pub fn cmd_selftest(a: &crate::util::Args) -> crate::util::R<i32> {
    let (pass, fail, msgs) = run(a.has("verbose") || a.has("all"));
    for m in &msgs {
        println!("{m}");
    }
    println!("自检 {pass} 通过 / {fail} 失败");
    if fail == 0 {
        Ok(0)
    } else {
        Ok(5)
    }
}
