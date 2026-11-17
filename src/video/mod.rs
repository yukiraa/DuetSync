//! Local YouTube player bridge (task T8).

mod server;

pub use server::{should_correct, ClientInfo, VideoBridge};

/// Extract a YouTube video id from an official YouTube URL or a bare id.
pub fn parse_youtube(input: &str) -> Option<String> {
    let input = input.trim();
    if is_video_id(input) {
        return Some(input.to_owned());
    }

    let rest = match input.split_once("://") {
        Some((scheme, rest))
            if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") =>
        {
            rest
        }
        Some(_) => return None,
        None => input,
    };
    let (host, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be"
    ) {
        return None;
    }
    let tail = format!("/{tail}");
    if host == "youtu.be" {
        return tail[1..]
            .split(['/', '?', '#'])
            .next()
            .filter(|id| is_video_id(id))
            .map(str::to_owned);
    }

    let (path, query) = tail.split_once('?').unwrap_or((&tail, ""));
    if path == "/watch" {
        return query
            .split('&')
            .filter_map(|part| part.split_once('='))
            .find_map(|(key, value)| (key == "v" && is_video_id(value)).then(|| value.to_owned()));
    }
    for prefix in ["/shorts/", "/embed/", "/live/"] {
        if let Some(id) = path.strip_prefix(prefix) {
            let id = id.split('/').next().unwrap_or_default();
            if is_video_id(id) {
                return Some(id.to_owned());
            }
        }
    }
    None
}

fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::parse_youtube;

    #[test]
    fn parses_youtube_urls() {
        for input in [
            "dQw4w9WgXcQ",
            "https://youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?x=1&v=dQw4w9WgXcQ&y=2",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=3",
            "https://youtube.com/shorts/dQw4w9WgXcQ",
            "https://youtube.com/embed/dQw4w9WgXcQ?autoplay=1",
            "https://youtube.com/live/dQw4w9WgXcQ",
            "youtube.com/watch?v=dQw4w9WgXcQ",
            " https://youtube.com/watch?v=dQw4w9WgXcQ ",
        ] {
            assert_eq!(
                parse_youtube(input).as_deref(),
                Some("dQw4w9WgXcQ"),
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_non_youtube_urls() {
        for input in [
            "",
            "dQw4w9WgXc",
            "dQw4w9WgXcQQ",
            "https://evil.test/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?v=bad",
            "https://youtube.com/channel/dQw4w9WgXcQ",
            "ftp://youtube.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert!(parse_youtube(input).is_none(), "{input}");
        }
    }
}
