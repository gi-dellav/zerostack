use crate::agent::tools::{TODO_LIST, TodoItem, TodoWriteArgs, WriteTodoList};
use compact_str::CompactString;
use rig::tool::Tool;

fn reset_todo_list() {
    let mut list = TODO_LIST
        .lock()
        .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());
    list.clear();
}

#[tokio::test]
async fn definition_name() {
    let tool = WriteTodoList::new(None, None);
    let def = tool.definition(String::new()).await;
    assert_eq!(def.name, "write_todo_list");
}

#[tokio::test]
async fn definition_description_non_empty() {
    let tool = WriteTodoList::new(None, None);
    let def = tool.definition(String::new()).await;
    assert!(!def.description.is_empty());
}

#[tokio::test]
async fn definition_parameters_has_required_fields() {
    let tool = WriteTodoList::new(None, None);
    let def = tool.definition(String::new()).await;
    let params = def.parameters.as_object().unwrap();
    assert!(params.contains_key("properties"));
    let props = params["properties"].as_object().unwrap();
    assert!(props.contains_key("todos"));
}

#[tokio::test]
async fn call_with_empty_todos() {
    reset_todo_list();
    let tool = WriteTodoList::new(None, None);
    let args = TodoWriteArgs { todos: vec![] };
    let result = tool.call(args).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.contains("cleared"), "got: {}", output);
}

#[tokio::test]
async fn call_formats_todo_items_with_icons() {
    reset_todo_list();
    let tool = WriteTodoList::new(None, None);
    let args = TodoWriteArgs {
        todos: vec![
            TodoItem {
                content: "High priority task".to_string(),
                status: CompactString::new("high"),
                priority: CompactString::new("high"),
            },
            TodoItem {
                content: "Completed task".to_string(),
                status: CompactString::new("completed"),
                priority: CompactString::new("medium"),
            },
            TodoItem {
                content: "In progress task".to_string(),
                status: CompactString::new("in_progress"),
                priority: CompactString::new("medium"),
            },
            TodoItem {
                content: "Cancelled task".to_string(),
                status: CompactString::new("cancelled"),
                priority: CompactString::new("low"),
            },
            TodoItem {
                content: "Low priority task".to_string(),
                status: CompactString::new("low"),
                priority: CompactString::new("low"),
            },
        ],
    };
    let result = tool.call(args).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.contains("[x]"));
    assert!(output.contains("[>]"));
    assert!(output.contains("[-]"));
    assert!(output.contains("[ ]"));
    assert!(output.contains("High priority task"));
    assert!(output.contains("Completed task"));
    assert!(output.contains("In progress task"));
    assert!(output.contains("Cancelled task"));
    assert!(output.contains("Low priority task"));
    assert!(output.contains("5 items"));
}

#[tokio::test]
async fn call_updates_global_todo_list() {
    reset_todo_list();
    let tool = WriteTodoList::new(None, None);
    let args = TodoWriteArgs {
        todos: vec![
            TodoItem {
                content: "Task 1".to_string(),
                status: CompactString::new("pending"),
                priority: CompactString::new("high"),
            },
            TodoItem {
                content: "Task 2".to_string(),
                status: CompactString::new("pending"),
                priority: CompactString::new("medium"),
            },
        ],
    };
    let _ = tool.call(args).await;

    let list = TODO_LIST
        .lock()
        .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].content, "Task 1");
    assert_eq!(list[1].content, "Task 2");
}

#[tokio::test]
async fn call_overwrites_previous_list() {
    reset_todo_list();
    let tool = WriteTodoList::new(None, None);

    let args1 = TodoWriteArgs {
        todos: vec![TodoItem {
            content: "First".to_string(),
            status: CompactString::new("pending"),
            priority: CompactString::new("high"),
        }],
    };
    let _ = tool.call(args1).await;

    let args2 = TodoWriteArgs {
        todos: vec![TodoItem {
            content: "Second".to_string(),
            status: CompactString::new("completed"),
            priority: CompactString::new("low"),
        }],
    };
    let _ = tool.call(args2).await;

    let list = TODO_LIST
        .lock()
        .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].content, "Second");
}
