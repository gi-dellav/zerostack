#[cfg(test)]
mod tests {
    use crate::extras::advisor::tool::AdvisorArgs;

    #[test]
    fn advisor_args_deserializes_empty() {
        let args: AdvisorArgs = serde_json::from_str("{}").unwrap();
        assert!(args.query.is_none());
    }

    #[test]
    fn advisor_args_deserializes_with_query() {
        let args: AdvisorArgs =
            serde_json::from_str(r#"{"query": "should I use channels?"}"#).unwrap();
        assert_eq!(args.query.unwrap(), "should I use channels?");
    }

    #[test]
    fn advisor_args_query_missing_is_none() {
        let args: AdvisorArgs = serde_json::from_str(r#"{}"#).unwrap();
        assert!(args.query.is_none());
    }

    #[test]
    fn advisor_args_extra_fields_ignored() {
        let args: AdvisorArgs = serde_json::from_str(r#"{"query": "test", "extra": 42}"#).unwrap();
        assert_eq!(args.query.unwrap(), "test");
    }

    #[test]
    fn advisor_tool_name_is_advisor() {
        use crate::extras::advisor::tool::AdvisorTool;
        use rig::tool::Tool;
        assert_eq!(AdvisorTool::NAME, "advisor");
    }
}
