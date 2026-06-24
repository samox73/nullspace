use crate::model::Reference;

/// Strip common DOI prefixes; return the bare DOI if the input looks like one.
pub fn normalize_doi(input: &str) -> Option<String> {
    let mut s = input.trim();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
        "DOI:",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim();
            break;
        }
    }
    if s.starts_with("10.") && s.contains('/') {
        Some(s.to_string())
    } else {
        None
    }
}

/// The clickable link: explicit URL if present, else the DOI as a doi.org URL.
pub fn reference_link(reference: &Reference) -> Option<String> {
    if let Some(url) = reference.url.as_ref() {
        let url = url.trim();
        if !url.is_empty() {
            return Some(url.to_string());
        }
    }
    let doi = reference.doi.as_ref()?.trim();
    if doi.is_empty() {
        None
    } else {
        Some(format!("https://doi.org/{doi}"))
    }
}

/// A single-line human-readable citation for display.
pub fn format_citation(reference: &Reference) -> String {
    let mut out = String::new();
    let authors = reference.authors.trim();
    if !authors.is_empty() {
        out.push_str(authors);
    }
    if let Some(year) = reference.year {
        if out.is_empty() {
            out.push_str(&year.to_string());
        } else {
            out.push_str(&format!(" ({year})"));
        }
    }
    let title = reference.title.trim();
    if !title.is_empty() {
        if out.is_empty() {
            out.push_str(title);
        } else {
            out.push_str(". ");
            out.push_str(title);
        }
    }
    if out.is_empty() {
        out.push_str("(untitled reference)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Reference;

    fn make(doi: Option<&str>, url: Option<&str>) -> Reference {
        Reference {
            authors: "Kohn, Sham".to_string(),
            year: Some(1965),
            title: "Phys. Rev. 140, A1133".to_string(),
            doi: doi.map(str::to_string),
            url: url.map(str::to_string),
        }
    }

    #[test]
    fn normalize_doi_strips_prefixes() {
        assert_eq!(
            normalize_doi("10.1103/PhysRev.140.A1133").as_deref(),
            Some("10.1103/PhysRev.140.A1133")
        );
        assert_eq!(
            normalize_doi("https://doi.org/10.1103/X").as_deref(),
            Some("10.1103/X")
        );
        assert_eq!(normalize_doi("doi:10.1103/X").as_deref(), Some("10.1103/X"));
        assert_eq!(normalize_doi("not a doi"), None);
        assert_eq!(normalize_doi(""), None);
    }

    #[test]
    fn reference_link_prefers_url_then_doi() {
        assert_eq!(
            reference_link(&make(Some("10.1/X"), Some("https://x.test"))).as_deref(),
            Some("https://x.test")
        );
        assert_eq!(
            reference_link(&make(Some("10.1/X"), None)).as_deref(),
            Some("https://doi.org/10.1/X")
        );
        assert_eq!(reference_link(&make(None, None)), None);
    }

    #[test]
    fn format_citation_combines_fields() {
        assert_eq!(
            format_citation(&make(None, None)),
            "Kohn, Sham (1965). Phys. Rev. 140, A1133"
        );
    }
}
