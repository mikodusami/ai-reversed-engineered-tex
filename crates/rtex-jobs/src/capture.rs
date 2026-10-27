//! Turn a job page into a `Posting`.
//!
//! Most career sites (Greenhouse, Lever, Ashby, Workday, iCIMS, LinkedIn
//! public pages…) embed a schema.org `JobPosting` as JSON-LD for search
//! engines; that gives title, company, location, pay and the description
//! without scraping layout. Pages without it fall back to their readable text.
//! The same functions handle HTML fetched by URL and HTML sent by the browser
//! extension from a page you have open.

use crate::workspace::Posting;
use serde_json::Value;
use std::process::Command;

/// Fetch a URL with curl (follows redirects, desktop user agent).
pub fn fetch(url: &str) -> Result<String, String> {
    let out = Command::new("/usr/bin/curl")
        .args(["-sSL", "--max-time", "25", "--compressed", "-A"])
        .arg("Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15")
        .arg(url)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("could not fetch {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Capture a job URL. Job boards that build their pages with JavaScript
/// (Workable, Greenhouse, Lever, Ashby) are read through their public job
/// APIs; anything else is fetched and its `<body>` text kept.
pub fn capture_url(url: &str) -> Result<Posting, String> {
    if let Some(p) = job_board(url) {
        return p;
    }
    from_html(&fetch(url)?, url)
}

fn get_json(api: &str) -> Result<Value, String> {
    let body = fetch(api)?;
    serde_json::from_str(&body).map_err(|_| format!("unexpected answer from {api}"))
}

fn board_posting(url: &str, title: String, company: String, location: String, parts: &[String]) -> Posting {
    let text: Vec<String> = parts.iter().map(|h| html_to_text(&html_unescape_basic(h))).filter(|t| !t.trim().is_empty()).collect();
    let body = format!("{title}\n{company}\n{location}\n\n{}", text.join("\n\n"));
    Posting { title, company, location, source: url.into(), apply_url: url.into(), body, ..Default::default() }
}

fn job_board(url: &str) -> Option<Result<Posting, String>> {
    let re = |p: &str| regex::Regex::new(p).unwrap();
    // apply.workable.com/<account>/j/<shortcode>
    if let Some(c) = re(r"apply\.workable\.com/(?:j/)?([^/?#]+)/j/([A-Za-z0-9]+)").captures(url) {
        let api = format!("https://apply.workable.com/api/v2/accounts/{}/jobs/{}", &c[1], &c[2]);
        return Some(get_json(&api).map(|v| {
            let loc = &v["location"];
            let location = [s(&loc["city"]), s(&loc["region"]), s(&loc["country"])].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", ");
            let remote = if v["remote"].as_bool().unwrap_or(false) { " (remote)" } else { "" };
            board_posting(url, s(&v["title"]), c[1].to_string(), format!("{location}{remote}"),
                &[s(&v["description"]), s(&v["requirements"]), s(&v["benefits"])])
        }));
    }
    // (job-)boards.greenhouse.io/<board>/jobs/<id>
    if let Some(c) = re(r"boards(?:\.eu)?\.greenhouse\.io/(?:embed/job_app\?for=)?([^/?#&]+)/jobs/(\d+)").captures(url) {
        let api = format!("https://boards-api.greenhouse.io/v1/boards/{}/jobs/{}", &c[1], &c[2]);
        return Some(get_json(&api).map(|v| {
            let company = Some(s(&v["company_name"])).filter(|n| !n.is_empty()).unwrap_or_else(|| c[1].to_string());
            board_posting(url, s(&v["title"]), company, s(&v["location"]["name"]), &[s(&v["content"])])
        }));
    }
    // jobs.lever.co/<site>/<id>
    if let Some(c) = re(r"jobs\.lever\.co/([^/?#]+)/([0-9a-f-]{36})").captures(url) {
        let api = format!("https://api.lever.co/v0/postings/{}/{}", &c[1], &c[2]);
        return Some(get_json(&api).map(|v| {
            let mut parts = vec![s(&v["description"])];
            for l in v["lists"].as_array().into_iter().flatten() {
                parts.push(format!("<h3>{}</h3><ul>{}</ul>", s(&l["text"]), s(&l["content"])));
            }
            parts.push(s(&v["additional"]));
            board_posting(url, s(&v["text"]), c[1].to_string(), s(&v["categories"]["location"]), &parts)
        }));
    }
    // jobs.ashbyhq.com/<org>/<id>
    if let Some(c) = re(r"jobs\.ashbyhq\.com/([^/?#]+)/([0-9a-f-]{36})").captures(url) {
        let api = format!("https://api.ashbyhq.com/posting-api/job-board/{}?includeCompensation=true", &c[1]);
        return Some(get_json(&api).and_then(|v| {
            let job = v["jobs"].as_array().into_iter().flatten().find(|j| s(&j["id"]) == c[2]).cloned()
                .ok_or_else(|| "the job is not on the company's Ashby board (closed?)".to_string())?;
            let pay = s(&job["compensation"]["compensationTierSummary"]);
            let mut p = board_posting(url, s(&job["title"]), c[1].to_string(), s(&job["location"]), &[s(&job["descriptionHtml"])]);
            if !pay.is_empty() {
                p.body = format!("Compensation: {pay}\n\n{}", p.body);
            }
            Ok(p)
        }));
    }
    None
}

/// The page as text: everything inside `<body>` (scripts and styles
/// dropped), which is what gets saved and sent to Claude. When the body has
/// almost no text (a page built by JavaScript), the schema.org JobPosting
/// that most career sites embed is used instead. Errors when neither has
/// enough text (a sign-in wall): paste the text, or capture it from the browser.
pub fn from_html(html: &str, url: &str) -> Result<Posting, String> {
    let title = tag_text(html, "title").unwrap_or_default();
    let body = body_text(html);
    if body.split_whitespace().count() >= 80 {
        return Ok(Posting { title: short_title(&title), source: url.into(), apply_url: url.into(), body, ..Default::default() });
    }
    if let Some(p) = json_ld(html, url) {
        return Ok(p);
    }
    Err("the page has too little text (sign-in wall or JavaScript-only page); paste its text instead".into())
}

/// Readable text of the whole `<body>` (or the whole document without one).
pub fn body_text(html: &str) -> String {
    let start = find_ci(html, "<body").and_then(|i| html[i..].find('>').map(|j| i + j + 1)).unwrap_or(0);
    let end = find_ci(&html[start..], "</body>").map(|j| start + j).unwrap_or(html.len());
    html_to_text(&html[start..end])
}

/// Page titles are often "Role - Company | Site"; keep it short for folder names.
fn short_title(t: &str) -> String {
    let t = t.trim();
    let mut out = String::new();
    for w in t.split_whitespace() {
        if out.len() + w.len() > 60 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

/// Posting from pasted text; the first non-empty line is used as the title
/// when none is given.
pub fn from_text(text: &str, title: &str, company: &str, location: &str, url: &str) -> Posting {
    let title = if title.is_empty() {
        text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(120).collect()
    } else {
        title.to_string()
    };
    Posting {
        title,
        company: company.into(),
        location: location.into(),
        source: url.into(),
        apply_url: url.into(),
        body: text.trim().into(),
        ..Default::default()
    }
}

/// The first schema.org JobPosting in the page's JSON-LD blocks.
pub fn json_ld(html: &str, url: &str) -> Option<Posting> {
    let mut rest = html;
    while let Some(i) = find_ci(rest, "application/ld+json") {
        rest = &rest[i..];
        let start = rest.find('>')? + 1;
        let end = find_ci(&rest[start..], "</script>")? + start;
        let raw = html_unescape_basic(rest[start..end].trim());
        rest = &rest[end..];
        let Ok(v) = serde_json::from_str::<Value>(&raw) else { continue };
        if let Some(jp) = find_job_posting(&v) {
            return Some(posting_from_ld(jp, url));
        }
    }
    None
}

fn find_job_posting(v: &Value) -> Option<&Value> {
    match v {
        Value::Array(a) => a.iter().find_map(find_job_posting),
        Value::Object(o) => {
            let is_job = match o.get("@type") {
                Some(Value::String(t)) => t == "JobPosting",
                Some(Value::Array(ts)) => ts.iter().any(|t| t == "JobPosting"),
                _ => false,
            };
            if is_job {
                return Some(v);
            }
            o.get("@graph").and_then(find_job_posting)
        }
        _ => None,
    }
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn posting_from_ld(jp: &Value, url: &str) -> Posting {
    let company = match &jp["hiringOrganization"] {
        Value::String(n) => n.clone(),
        o => s(&o["name"]),
    };
    let locs: Vec<String> = match &jp["jobLocation"] {
        Value::Array(a) => a.iter().map(location).filter(|l| !l.is_empty()).collect(),
        v if v.is_object() => vec![location(v)],
        _ => vec![],
    };
    let mut loc = locs.join("; ");
    if s(&jp["jobLocationType"]).eq_ignore_ascii_case("TELECOMMUTE") {
        loc = if loc.is_empty() { "Remote".into() } else { format!("{loc} (remote)") };
    }
    let pay = salary(&jp["baseSalary"]);
    let mut body = html_to_text(&html_unescape_basic(&s(&jp["description"])));
    let extras: Vec<String> = [
        ("Employment type", s(&jp["employmentType"])),
        ("Posted", s(&jp["datePosted"])),
        ("Apply by", s(&jp["validThrough"])),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| format!("{k}: {v}"))
    .collect();
    if !extras.is_empty() {
        body = format!("{}\n\n{body}", extras.join(". "));
    }
    Posting {
        title: s(&jp["title"]),
        company,
        location: loc,
        pay,
        source: url.into(),
        apply_url: url.into(),
        requisition: s(&jp["identifier"]["value"]),
        body,
    }
}

fn location(v: &Value) -> String {
    let a = &v["address"];
    if a.is_string() {
        return s(a);
    }
    let country = match &a["addressCountry"] {
        Value::Object(_) => s(&a["addressCountry"]["name"]),
        c => s(c),
    };
    [s(&a["addressLocality"]), s(&a["addressRegion"]), country]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn salary(v: &Value) -> String {
    if v.is_null() {
        return String::new();
    }
    let cur = s(&v["currency"]);
    let val = &v["value"];
    let unit = s(&val["unitText"]).to_lowercase();
    let (lo, hi) = (s(&val["minValue"]), s(&val["maxValue"]));
    let single = s(&val["value"]);
    let amount = match (lo.is_empty(), hi.is_empty()) {
        (false, false) => format!("{lo}–{hi}"),
        (false, true) => lo,
        (true, false) => hi,
        _ => single,
    };
    if amount.is_empty() {
        return String::new();
    }
    format!("{} {amount}{}", cur, if unit.is_empty() { String::new() } else { format!(" per {unit}") }).trim().into()
}

fn tag_text(html: &str, tag: &str) -> Option<String> {
    let i = find_ci(html, &format!("<{tag}"))?;
    let start = html[i..].find('>')? + i + 1;
    let end = find_ci(&html[start..], &format!("</{tag}>"))? + start;
    Some(html_to_text(&html[start..end]))
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Readable text from HTML: drops script/style, turns blocks into lines,
/// list items into "- " bullets and headings into "## ".
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let b = html.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            let Some(close) = html[i..].find('>') else { break };
            let tag = &html[i + 1..i + close];
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            let closing = tag.starts_with('/');
            i += close + 1;
            if !closing && (name == "script" || name == "style" || name == "noscript" || name == "svg") {
                if let Some(end) = find_ci(&html[i..], &format!("</{name}")) {
                    i += end;
                }
                continue;
            }
            match name.as_str() {
                "li" if !closing => out.push_str("\n- "),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !closing => out.push_str("\n\n## "),
                "br" => out.push('\n'),
                "p" | "div" | "ul" | "ol" | "section" | "tr" | "table" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => out.push_str("\n\n"),
                "td" | "th" => out.push(' '),
                _ => {}
            }
            continue;
        }
        let next = html[i..].find('<').map(|n| i + n).unwrap_or(b.len());
        out.push_str(&html_unescape_basic(&html[i..next]));
        i = next;
    }
    // Collapse whitespace: single spaces inside lines, at most one blank line.
    let mut text = String::new();
    let mut blank = 0;
    for line in out.lines() {
        let l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() || l == "-" || l == "##" {
            blank += 1;
            continue;
        }
        if !text.is_empty() {
            text.push_str(if blank > 0 && !l.starts_with("- ") { "\n\n" } else { "\n" });
        }
        text.push_str(&l);
        blank = 0;
    }
    text
}

fn html_unescape_basic(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        let rep = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" | "#160" => Some(' '),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "rsquo" => Some('’'),
            "lsquo" => Some('‘'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "bull" => Some('•'),
            "hellip" => Some('…'),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match rep {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head><title>Ignored</title>
<script type="application/ld+json">{"@context":"https://schema.org","@type":"JobPosting","title":"Software Engineer",
"hiringOrganization":{"@type":"Organization","name":"TickPick"},
"jobLocation":{"@type":"Place","address":{"addressLocality":"New York","addressRegion":"NY","addressCountry":"US"}},
"baseSalary":{"currency":"USD","value":{"minValue":100000,"maxValue":130000,"unitText":"YEAR"}},
"employmentType":"FULL_TIME",
"description":"&lt;p&gt;Build &amp;amp; ship.&lt;/p&gt;&lt;ul&gt;&lt;li&gt;React&lt;/li&gt;&lt;li&gt;C#/.NET&lt;/li&gt;&lt;/ul&gt;"}</script>
</head><body>nav</body></html>"#;

    #[test]
    fn reads_json_ld_job_posting() {
        let p = json_ld(PAGE, "https://x/job").unwrap();
        assert_eq!(p.title, "Software Engineer");
        assert_eq!(p.company, "TickPick");
        assert_eq!(p.location, "New York, NY, US");
        assert_eq!(p.pay, "USD 100000–130000 per year");
        assert!(p.body.contains("Build & ship."), "{}", p.body);
        assert!(p.body.contains("- React\n- C#/.NET"), "{}", p.body);
        assert!(p.body.starts_with("Employment type: FULL_TIME"));
    }

    #[test]
    fn graph_and_arrays() {
        let html = r#"<script type="application/ld+json">{"@graph":[{"@type":"WebPage"},{"@type":["JobPosting"],"title":"T","hiringOrganization":"Acme","description":"x"}]}</script>"#;
        let p = json_ld(html, "u").unwrap();
        assert_eq!((p.title.as_str(), p.company.as_str()), ("T", "Acme"));
    }

    #[test]
    fn body_text_first_then_json_ld_then_wall() {
        let words = "word ".repeat(100);
        let html = format!("<html><head><title>Job | Acme</title><script>var x=1;</script></head><body><nav>Menu</nav><main><h2>About</h2><p>{words}</p></main><script>track()</script></body></html>");
        let p = from_html(&html, "u").unwrap();
        assert_eq!(p.title, "Job | Acme");
        assert!(p.body.starts_with("Menu"), "{}", p.body);
        assert!(!p.body.contains("track()") && !p.body.contains("var x"));
        // A JavaScript-only page: the embedded JobPosting is used.
        assert_eq!(from_html(PAGE, "https://x/job").unwrap().company, "TickPick");
        assert!(from_html("<body><p>Sign in</p></body>", "u").is_err());
    }

    #[test]
    fn entities() {
        assert_eq!(html_unescape_basic("a &amp; b &#8212; &#x2014; &nbsp;c &bogus;"), "a & b — —  c &bogus;");
    }
}
