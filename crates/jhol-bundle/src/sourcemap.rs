use crate::errors::BundleResult;

const VLQ_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode a VLQ value. Base64-VLQ encoding uses 6 bits per char, continuation bit = 0x20.
fn encode_vlq(mut value: i32, out: &mut String) {
    let mut sign = 0;
    if value < 0 {
        sign = 1;
        value = -value;
    }
    let mut vlq = (value << 1) | sign;
    loop {
        let mut digit = (vlq & 0x1f) as u8;
        vlq >>= 5;
        if vlq > 0 {
            digit |= 0x20;
        }
        out.push(VLQ_CHARS[digit as usize] as char);
        if vlq == 0 {
            break;
        }
    }
}

pub fn generate_sourcemap(
    sources: &[String],
    sources_content: &[String],
    output_file: &str,
) -> BundleResult<String> {
    let mut m = String::from("{\"version\":3,\"file\":\"");
    m.push_str(&output_file.replace('\\', "\\\\").replace('"', "\\\""));
    m.push_str("\",\"sources\":[");
    for (i, s) in sources.iter().enumerate() {
        if i > 0 {
            m.push(',');
        }
        m.push('"');
        m.push_str(&s.replace('\\', "\\\\").replace('"', "\\\""));
        m.push('"');
    }
    m.push_str("],\"sourcesContent\":[");
    for (i, c) in sources_content.iter().enumerate() {
        if i > 0 {
            m.push(',');
        }
        let v = serde_json::to_string(c).unwrap_or_else(|_| "\"\"".into());
        m.push_str(&v);
    }
    m.push_str("],\"names\":[],\"mappings\":\"");
    let mut prev_gl = 0;
    for (si, content) in sources_content.iter().enumerate() {
        for (line_no, _) in content.lines().enumerate() {
            let gl = line_no as i32;
            encode_vlq(gl - prev_gl, &mut m);
            encode_vlq(0, &mut m); // col
            encode_vlq(si as i32, &mut m); // source
            encode_vlq(gl, &mut m); // orig line
            encode_vlq(0, &mut m); // orig col
            m.push(';');
            prev_gl = gl;
        }
    }
    m.push_str("\"}");
    Ok(m)
}

pub fn inline_sourcemap(code: &str, sourcemap_json: &str) -> BundleResult<String> {
    use base64::Engine;
    let enc = base64::engine::general_purpose::STANDARD.encode(sourcemap_json);
    Ok(format!(
        "{}\n//# sourceMappingURL=data:application/json;charset=utf-8;base64,{}",
        code, enc
    ))
}

pub fn append_sourcemap_comment(code: &str, sourcemap_path: &str) -> String {
    format!("{}\n//# sourceMappingURL={}", code, sourcemap_path)
}
