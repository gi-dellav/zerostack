#[cfg(test)]
mod tests {
    use crate::extras::adviser::tool::AdviserArgs;

    #[test]
    fn adviser_args_deserializes_empty() {
        let args: AdviserArgs = serde_json::from_str("{}").unwrap();
        assert!(args.query.is_none());
    }

    #[test]
    fn adviser_args_deserializes_with_query() {
        let args: AdviserArgs =
            serde_json::from_str(r#"{"query": "should I use channels?"}"#).unwrap();
        assert_eq!(args.query.unwrap(), "should I use channels?");
    }

    #[test]
    fn adviser_args_query_missing_is_none() {
        let args: AdviserArgs = serde_json::from_str(r#"{}"#).unwrap();
        assert!(args.query.is_none());
    }

    #[test]
    fn adviser_args_extra_fields_ignored() {
        let args: AdviserArgs =
            serde_json::from_str(r#"{"query": "test", "extra": 42}"#).unwrap();
        assert_eq!(args.query.unwrap(), "test");
    }

    #[test]
    fn adviser_tool_name_is_adviser() {
        use rig::tool::Tool;
        use crate::extras::adviser::tool::AdviserTool;
        assert_eq!(AdviserTool::NAME, "adviser");
    }
}
