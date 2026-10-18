use super::{
    api::Api,
    auth,
    config::{self, flag, text},
    output::{self, Error, Result},
};
use clap::{
    error::{ContextKind, ContextValue, ErrorKind},
    Arg, ArgAction, ArgMatches, Command,
};
use reqwest::Method;
use serde_json::{json, Value};

const TASK: &[&str] = &[
    "id",
    "number",
    "title",
    "projectId",
    "status",
    "priority",
    "userId",
    "assigneeId",
    "assigneeName",
    "startDate",
    "dueDate",
];
const PROJECT: &[&str] = &["id", "name", "slug", "icon", "workspaceId", "description", "archivedAt"];
const LABEL: &[&str] = &["id", "name", "color", "workspaceId", "taskId"];
const COMMENT: &[&str] = &["id", "taskId", "userId", "content", "createdAt", "updatedAt"];
const FIELD: &[&str] = &[
    "id",
    "projectId",
    "name",
    "type",
    "required",
    "defaultValue",
    "options",
    "position",
];
const FIELD_VALUE: &[&str] = &[
    "id",
    "taskId",
    "fieldId",
    "value",
    "fieldName",
    "fieldType",
    "fieldOptions",
    "fieldPosition",
];
const INPUT_ERROR: &str = "Input is missing a required field, contains an unknown field or has an invalid value";
const RELATION: &[&str] = &["id", "sourceTaskId", "targetTaskId", "relationType", "createdAt"];
const SEARCH_TYPES: &[&str] = &["all", "tasks", "projects", "workspaces", "comments", "activities"];

fn option(name: &'static str) -> Arg {
    // Commander consumed the next argument as a value even when it was a flag.
    let arg = Arg::new(name)
        .long(name)
        .num_args(1)
        .allow_hyphen_values(true)
        .action(ArgAction::Set);
    match name {
        "workspace" => arg.value_name("ID").help(config::WORKSPACE_ID_HELP),
        "project" => arg.value_name("ID").help(config::PROJECT_ID_HELP),
        "task" => arg.value_name("ID").help("Task id returned by task list or search"),
        "status" => arg.value_name("SLUG").help(config::STATUS_SLUG_HELP),
        "priority" => arg.help("no-priority, low, medium, high, urgent"),
        "due" => arg
            .long("due-date")
            .visible_alias("due")
            .value_name("DATE")
            .help("Due date: YYYY-MM-DD (midnight UTC) or ISO 8601 with timezone"),
        "type" => arg.help(format!("Search category: {} (default: all)", SEARCH_TYPES.join(", "))),
        _ => arg,
    }
}
fn boolean(name: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue)
}
fn id(name: &'static str) -> Command {
    Command::new(name).arg(Arg::new("id").required(true))
}
fn options(mut command: Command, names: &[&'static str]) -> Command {
    for name in names {
        command = command.arg(option(name));
    }
    command
}
fn required(command: Command, name: &'static str) -> Command {
    command.arg(option(name).required(true))
}
fn group(name: &'static str) -> Command {
    Command::new(name)
        .subcommand_required(true)
        .arg_required_else_help(true)
}

pub fn parser() -> Command {
    let task_create = options(
        required(Command::new("create"), "project"),
        &[
            "file",
            "title",
            "status",
            "description",
            "description-file",
            "priority",
            "assignee",
            "start",
            "due",
        ],
    )
    .arg(option("field").action(ArgAction::Append))
    .after_help("Creation requires both --title and --status, or title and status fields in --file JSON.");
    let task_update = options(
        id("update"),
        &[
            "title",
            "description",
            "description-file",
            "status",
            "priority",
            "assignee",
            "due",
        ],
    )
    .arg(boolean("unassign"))
    .arg(boolean("clear-due"));
    let task = group("task")
        .subcommand(options(
            required(Command::new("list"), "project"),
            &[
                "page",
                "limit",
                "status",
                "priority",
                "assignee",
                "due-before",
                "due-after",
            ],
        ))
        .subcommand(id("get"))
        .subcommand(task_create)
        .subcommand(task_update)
        .subcommand(required(id("move"), "project").arg(option("status")))
        .subcommand(id("delete"))
        .subcommand(id("restore"))
        .subcommand(options(Command::new("trash"), &["page", "limit"]))
        .subcommand(id("activity"))
        .subcommand(id("fields").about("Get values for a task; project field definitions: project fields <ID>"))
        .subcommand(required(Command::new("columns"), "project").about("Get project columns and valid --status slugs"))
        .subcommand(group("field").subcommand(required(required(id("set"), "field"), "value")));
    task.bin_name("apeiron task")
        .display_name("apeiron task")
        .args_override_self(true)
        .about("Manage tasks, projects and workspaces as the current user")
        .after_help("Find resource IDs before operating on tasks:\n  apeiron task workspace list --json\n  apeiron task project list --workspace <ID> --json\n  apeiron task list --project <ID> --json\n  apeiron task project columns <ID> --json\n  apeiron task create --project <ID> --title <TEXT> --status <SLUG> --json")
        .version(env!("CARGO_PKG_VERSION"))
        .propagate_version(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("version")
                .long("version")
                .short('V')
                .global(true)
                .action(ArgAction::Version),
        )
        .arg(option("api-url").global(true))
        .arg(option("workspace").global(true))
        .arg(option("timeout-ms").global(true).default_value("10000"))
        .arg(boolean("json").global(true))
        .subcommand(Command::new("whoami"))
        .subcommand(
            group("auth")
                .about("Inspect or manage your independent CLI login")
                .subcommand(Command::new("login").arg(boolean("no-open")))
                .subcommand(Command::new("status"))
                .subcommand(Command::new("logout")),
        )
        .subcommand(
            group("workspace")
                .subcommand(Command::new("list"))
                .subcommand(Command::new("members")),
        )
        .subcommand(
            group("project")
                .subcommand(Command::new("list").arg(boolean("include-archived")))
                .subcommand(id("get"))
                .subcommand(required(
                    required(required(Command::new("create"), "name"), "slug"),
                    "icon",
                ))
                .subcommand(id("delete").about("Permanently delete a project and its contents; prefer archive").arg(boolean("confirm")))
                .subcommand(id("archive"))
                .subcommand(id("unarchive"))
                .subcommand(id("columns"))
                .subcommand(id("fields").about("Get custom field definitions for a project")),
        )
        .subcommand(
            group("relation")
                .about("Link existing tasks; subtask uses --source as parent and --target as child")
                .subcommand(required(Command::new("list"), "task"))
                .subcommand(required(required(required(Command::new("create"), "source"), "target"), "relation-type"))
                .subcommand(id("delete")),
        )
        .subcommand(options(
            Command::new("search").arg(Arg::new("query").required(true).help("Nonempty search text")),
            &["project", "type", "limit"],
        ))
        .subcommand(
            group("comment")
                .subcommand(required(Command::new("list"), "task"))
                .subcommand(options(required(Command::new("add"), "task"), &["body", "body-file"]))
                .subcommand(options(id("update"), &["body", "body-file"])),
        )
        .subcommand(
            group("label")
                .subcommand(Command::new("list").arg(option("task")))
                .subcommand(required(required(Command::new("create"), "name"), "color"))
                .subcommand(required(id("attach"), "task"))
                .subcommand(required(id("detach"), "task")),
        )
}

pub fn argument_error(error: &clap::Error) -> Error {
    // MissingRequiredArgument contains parser-defined argument names. The full
    // clap error/usage can include user input, so never echo it into tool output.
    if error.kind() == ErrorKind::MissingRequiredArgument {
        if let Some(ContextValue::Strings(args)) = error.get(ContextKind::InvalidArg) {
            let hint = if args.iter().any(|arg| arg == "--project <ID>") {
                config::PROJECT_ID_HELP
            } else {
                "Use --help for available options."
            };
            return Error::invalid(format!("Missing required arguments: {}. {hint}", args.join(", ")));
        }
    }
    Error::invalid("Invalid command arguments; use --help for available options")
}

pub async fn execute(root: &ArgMatches, api: &mut Api) -> Result<(Value, Option<Value>)> {
    let (group, args) = root.subcommand().ok_or_else(Error::internal)?;
    if group == "whoami" {
        return Ok((whoami(api).await?, None));
    }
    if group == "search" {
        return search(args, api).await;
    }
    // Task operations live at the root; related resources retain their groups.
    let (group, operation, args) = match group {
        "auth" | "workspace" | "project" | "comment" | "label" | "relation" => {
            let (operation, args) = args.subcommand().ok_or_else(Error::internal)?;
            (group, operation, args)
        }
        operation => ("task", operation, args),
    };
    let id = text(args, "id").unwrap_or("");
    let mut meta = None;
    let data = match (group, operation) {
        ("auth", "login") => auth::login(&api.http, &api.config, !flag(args, "no-open")).await?,
        ("auth", "logout") => auth::logout(&api.http, &api.config).await?,
        ("auth", "status") => {
            let mut result = whoami(api).await?;
            result["credentialSource"] = json!(api.credential().await?.source);
            result
        }
        ("workspace", "list") => json!({"items":whoami(api).await?["workspaces"]}),
        ("workspace", "members") => {
            let workspace = api.config.required(args, "workspace")?.to_owned();
            output::list(
                &api.get(&["workspace", &workspace, "members"], &[]).await?,
                &["id", "name", "role"],
            )?
        }
        ("project", "list") => {
            let mut query = vec![("workspaceId", api.config.required(args, "workspace")?.to_owned())];
            if flag(args, "include-archived") {
                query.push(("includeArchived", "true".into()));
            }
            output::list(&api.get(&["project"], &query).await?, PROJECT)?
        }
        ("project", "get") => output::project(&api.get(&["project", id], &[]).await?, PROJECT)?,
        ("project", "create") => {
            let body = json!({"workspaceId":api.config.required(args,"workspace")?,"name":api.config.required(args,"name")?,"slug":api.config.required(args,"slug")?,"icon":api.config.required(args,"icon")?});
            output::project(
                &api.request(Method::POST, &["project"], &[], Some(body)).await?,
                PROJECT,
            )?
        }
        ("project", "archive" | "unarchive") => output::project(
            &api.request(Method::PUT, &["project", id, operation], &[], None).await?,
            PROJECT,
        )?,
        ("project", "delete") => {
            if !flag(args, "confirm") {
                return Err(Error::invalid("Project deletion is permanent and deletes its contents; pass --confirm, or use project archive <ID>"));
            }
            output::project(
                &api.request(Method::DELETE, &["project", id], &[], None).await?,
                PROJECT,
            )?
        }
        ("task", "columns") => {
            let project = api.config.required(args, "project")?.to_owned();
            output::list(
                &api.get(&["column", &project], &[]).await?,
                &["id", "name", "slug", "isFinal", "position"],
            )?
        }
        ("relation", "list") => {
            let task = api.config.required(args, "task")?.to_owned();
            output::list(&api.get(&["task-relation", &task], &[]).await?, RELATION)?
        }
        ("relation", "create") => {
            let source = api.config.required(args, "source")?;
            let target = api.config.required(args, "target")?;
            let kind = api.config.required(args, "relation-type")?;
            if !["subtask", "blocks", "related"].contains(&kind) || source == target {
                return Err(Error::invalid(
                    "--relation-type must be subtask, blocks or related; --source and --target must differ",
                ));
            }
            let body = json!({"sourceTaskId":source,"targetTaskId":target,"relationType":kind});
            output::project(
                &api.request(Method::POST, &["task-relation"], &[], Some(body)).await?,
                RELATION,
            )?
        }
        ("relation", "delete") => output::project(
            &api.request(Method::DELETE, &["task-relation", id], &[], None).await?,
            RELATION,
        )?,
        ("project", "columns") => output::list(
            &api.get(&["column", id], &[]).await?,
            &["id", "name", "slug", "isFinal", "position"],
        )?,
        ("project", "fields") => output::list(&api.get(&["custom-field", "project", id], &[]).await?, FIELD)?,
        ("task", "list") => {
            let (data, paging) = task_list(args, api).await?;
            meta = Some(paging);
            data
        }
        ("task", "get") => output::project(
            &api.get(&["task", id], &[]).await?,
            &[TASK, &["description", "createdAt"]].concat(),
        )?,
        ("task", "create") => task_create(args, api).await?,
        ("task", "update") => task_update(args, api).await?,
        ("task", "move") => {
            let mut body = json!({"destinationProjectId":api.config.required(args,"project")?});
            optional(&mut body, "destinationStatus", text(args, "status"));
            let result = api.request(Method::PUT, &["task", "move", id], &[], Some(body)).await?;
            output::nonempty(&result["sourceProjectId"])?;
            output::nonempty(&result["destinationProjectId"])?;
            output::project(&result["task"], TASK)?
        }
        ("task", "delete") => {
            let mut result = output::project(
                &api.request(Method::DELETE, &["task", id], &[], None).await?,
                &["id", "title", "deletedAt"],
            )?;
            result["deletionMode"] = json!("soft");
            result
        }
        ("task", "restore") => output::project(
            &api.request(Method::POST, &["task", id, "restore"], &[], None).await?,
            TASK,
        )?,
        ("task", "trash") => {
            let page = config::positive(text(args, "page"), 1, 1_000_000)?;
            let limit = config::positive(text(args, "limit"), 50, 100)?;
            let workspace = api.config.required(args, "workspace")?.to_owned();
            let result = api
                .get(
                    &["task", "trash", &workspace],
                    &[("page", page.to_string()), ("limit", limit.to_string())],
                )
                .await?;
            let p = output::integer(&result["pagination"]["page"], 1, u64::MAX)?;
            let l = output::integer(&result["pagination"]["limit"], 1, u64::MAX)?;
            let total = output::integer(&result["pagination"]["total"], 0, u64::MAX)?;
            meta = Some(json!({"page":p,"limit":l,"total":total,"hasMore":(p as f64)*(l as f64)<total as f64}));
            output::list(
                &result["tasks"],
                &["id", "title", "number", "projectId", "projectName", "deletedAt"],
            )?
        }
        ("task", "activity") => output::list(
            &api.get(&["activity", id], &[]).await?,
            &["id", "taskId", "type", "content", "userId", "createdAt"],
        )?,
        ("task", "fields") => output::list(&api.get(&["custom-field", "task", id], &[]).await?, FIELD_VALUE)?,
        ("task", "field") => {
            let (_, args) = args.subcommand().ok_or_else(Error::internal)?;
            let body = json!({"taskId":text(args,"id"),"fieldId":api.config.required(args,"field")?,"value":text(args,"value").unwrap_or("")});
            output::project(
                &api.request(Method::PUT, &["custom-field", "value"], &[], Some(body))
                    .await?,
                FIELD_VALUE,
            )?
        }
        ("comment", "list") => {
            let task = api.config.required(args, "task")?.to_owned();
            output::list(&api.get(&["comment", &task], &[]).await?, COMMENT)?
        }
        ("comment", "add" | "update") => {
            let content = config::input(text(args, "body"), text(args, "body-file"))
                .await?
                .filter(|s| !s.is_empty())
                .ok_or_else(|| Error::invalid("A nonempty --body or --body-file is required"))?;
            let resource = if operation == "add" {
                api.config.required(args, "task")?
            } else {
                id
            }
            .to_owned();
            let method = if operation == "add" { Method::POST } else { Method::PUT };
            output::project(
                &api.request(method, &["comment", &resource], &[], Some(json!({"content":content})))
                    .await?,
                COMMENT,
            )?
        }
        ("label", "list") => {
            let (scope, resource) = if let Some(task) = text(args, "task").filter(|s| !s.is_empty()) {
                ("task", task)
            } else {
                ("workspace", api.config.required(args, "workspace")?)
            };
            let resource = resource.to_owned();
            output::list(&api.get(&["label", scope, &resource], &[]).await?, LABEL)?
        }
        ("label", "create") => {
            let body = json!({"name":api.config.required(args,"name")?,"color":api.config.required(args,"color")?,"workspaceId":api.config.required(args,"workspace")?});
            output::project(&api.request(Method::POST, &["label"], &[], Some(body)).await?, LABEL)?
        }
        ("label", "attach") => {
            let body = json!({"taskId":api.config.required(args,"task")?});
            output::project(
                &api.request(Method::PUT, &["label", id, "task"], &[], Some(body))
                    .await?,
                LABEL,
            )?
        }
        ("label", "detach") => {
            let label = api.get(&["label", id], &[]).await?;
            output::object(&label)?;
            if label["taskId"].as_str() != Some(api.config.required(args, "task")?) {
                return Err(Error::new("CONFLICT", "Label is not attached to the requested task", 6));
            }
            output::project(
                &api.request(Method::DELETE, &["label", id, "task"], &[], None).await?,
                LABEL,
            )?
        }
        _ => return Err(Error::internal()),
    };
    Ok((data, meta))
}

async fn whoami(api: &mut Api) -> Result<Value> {
    output::identity(&api.get(&["me"], &[]).await?)
}
async fn assignee(api: &mut Api, value: Option<&str>) -> Result<Option<String>> {
    Ok(if value == Some("me") {
        Some(output::string(&whoami(api).await?["user"]["id"])?.into())
    } else {
        value.map(str::to_owned)
    })
}
fn optional(body: &mut Value, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        body[key] = json!(value);
    }
}
fn priority(value: &str) -> Result<&str> {
    if ["no-priority", "low", "medium", "high", "urgent"].contains(&value) {
        Ok(value)
    } else {
        Err(Error::invalid(
            "--priority must be no-priority, low, medium, high or urgent",
        ))
    }
}

async fn search(args: &ArgMatches, api: &mut Api) -> Result<(Value, Option<Value>)> {
    let kind = text(args, "type").unwrap_or("all");
    if !SEARCH_TYPES.contains(&kind) {
        return Err(Error::invalid(format!(
            "--type must be one of: {}",
            SEARCH_TYPES.join(", ")
        )));
    }
    let limit = config::positive(text(args, "limit"), 20, 50)?;
    let mut query = vec![
        ("q", text(args, "query").unwrap_or("").into()),
        ("type", kind.into()),
        ("workspaceId", api.config.required(args, "workspace")?.into()),
        ("limit", limit.to_string()),
    ];
    if let Some(project) = text(args, "project") {
        query.push(("projectId", project.into()));
    }
    let response = api.get(&["search"], &query).await?;
    let total = output::integer(&response["totalCount"], 0, u64::MAX)?;
    Ok((
        output::list(
            &response["results"],
            &[
                "id",
                "type",
                "title",
                "projectId",
                "projectName",
                "workspaceId",
                "taskNumber",
                "priority",
                "status",
            ],
        )?,
        Some(json!({"limit":limit,"totalCount":total})),
    ))
}

async fn task_list(args: &ArgMatches, api: &mut Api) -> Result<(Value, Value)> {
    let page = config::positive(text(args, "page"), 1, 1_000_000)?;
    let limit = config::positive(text(args, "limit"), 50, 100)?;
    let mut query = vec![
        ("page", page.to_string()),
        ("limit", limit.to_string()),
        ("sortBy", "number".into()),
        ("sortOrder", "asc".into()),
    ];
    for (option, key) in [
        ("status", "status"),
        ("priority", "priority"),
        ("assignee", "assigneeId"),
        ("due-before", "dueBefore"),
        ("due-after", "dueAfter"),
    ] {
        let value = text(args, option);
        if option.starts_with("due-") {
            config::date(value)?;
        }
        if let Some(value) = value {
            query.push((key, value.into()));
        }
    }
    let project = api.config.required(args, "project")?.to_owned();
    let response = api.get(&["task", "tasks", &project], &query).await?;
    let board = &response["data"];
    let paging = &response["pagination"];
    let p = output::integer(&paging["page"], 1, u64::MAX)?;
    let l = output::integer(&paging["pageSize"], 1, 100)?;
    let total = output::integer(&paging["total"], 0, u64::MAX)?;
    let pages = output::integer(&paging["totalPages"], 1, u64::MAX)?;
    let mut records = Vec::new();
    for column in output::array(&board["columns"])? {
        records.extend(output::array(&column["tasks"])?);
    }
    records.extend(output::array(&board["archivedTasks"])?);
    records.extend(output::array(&board["plannedTasks"])?);
    let mut unique = std::collections::HashMap::new();
    for task in records {
        let id = output::string(&task["id"])?;
        let number = output::object(task)?.get("number").ok_or_else(Error::response)?;
        if !number.is_null() && !number.is_number() {
            return Err(Error::response());
        }
        unique.insert(id, task);
    }
    let mut unique: Vec<_> = unique.into_values().collect();
    unique.sort_by(|a, b| {
        a["number"]
            .as_f64()
            .unwrap_or(f64::INFINITY)
            .total_cmp(&b["number"].as_f64().unwrap_or(f64::INFINITY))
            .then_with(|| {
                a["id"]
                    .as_str()
                    .unwrap_or("")
                    .encode_utf16()
                    .cmp(b["id"].as_str().unwrap_or("").encode_utf16())
            })
    });
    Ok((
        json!({"items":unique.into_iter().map(|t|output::project(t,TASK)).collect::<Result<Vec<_>>>()?}),
        json!({"page":p,"limit":l,"total":total,"hasMore":p<pages}),
    ))
}

async fn task_create(args: &ArgMatches, api: &mut Api) -> Result<Value> {
    let mut body = if let Some(file) = text(args, "file") {
        if [
            "title",
            "status",
            "description",
            "description-file",
            "priority",
            "assignee",
            "start",
            "due",
            "field",
        ]
        .into_iter()
        .any(|key| args.value_source(key) == Some(clap::parser::ValueSource::CommandLine))
        {
            return Err(Error::invalid(
                "--file cannot be combined with task creation field options",
            ));
        }
        let raw = config::input(None, Some(file))
            .await
            .map_err(|_| Error::invalid("Creation file must contain valid JSON"))?
            .unwrap_or_default();
        serde_json::from_str::<Value>(&raw).map_err(|_| Error::invalid("Creation file must contain valid JSON"))?
    } else {
        let mut fields = Vec::new();
        if let Some(values) = args.get_many::<String>("field") {
            for field in values {
                let (id, value) = field
                    .split_once('=')
                    .filter(|(id, _)| !id.is_empty())
                    .ok_or_else(|| Error::invalid("Custom fields use fieldId=value"))?;
                fields.push(json!({"fieldId":id,"value":value}));
            }
        }
        let mut data = json!({"description":config::input(text(args,"description"),text(args,"description-file")).await?.unwrap_or_default(),"priority":text(args,"priority").unwrap_or("no-priority"),"customFields":fields});
        for (option, key) in [
            ("title", "title"),
            ("status", "status"),
            ("assignee", "userId"),
            ("start", "startDate"),
            ("due", "dueDate"),
        ] {
            optional(&mut data, key, text(args, option));
        }
        data
    };
    let obj = body.as_object().ok_or_else(|| Error::invalid(INPUT_ERROR))?;
    if obj.keys().any(|k| {
        ![
            "title",
            "description",
            "status",
            "priority",
            "userId",
            "startDate",
            "dueDate",
            "customFields",
        ]
        .contains(&k.as_str())
    }) {
        return Err(Error::invalid(INPUT_ERROR));
    }
    for key in ["title", "status", "description", "priority"] {
        let invalid = || match key {
            "title" => {
                Error::invalid("Task creation requires a nonempty title: use --title or a title field in --file JSON.")
            }
            "status" => Error::invalid(format!(
                "Task creation requires a nonempty status: use --status or a status field in --file JSON. {}",
                config::STATUS_SLUG_HELP
            )),
            _ => Error::invalid(INPUT_ERROR),
        };
        let value = body[key].as_str().ok_or_else(invalid)?;
        if key != "description" && value.is_empty() {
            return Err(invalid());
        }
    }
    priority(body["priority"].as_str().unwrap_or(""))?;
    for key in ["userId", "startDate", "dueDate"] {
        if let Some(value) = obj.get(key) {
            let s = value.as_str().ok_or_else(|| Error::invalid(INPUT_ERROR))?;
            if key == "userId" && s.is_empty() {
                return Err(Error::invalid(INPUT_ERROR));
            }
        }
    }
    if let Some(fields) = obj.get("customFields") {
        let mut seen = std::collections::HashSet::new();
        for field in fields.as_array().ok_or_else(|| Error::invalid(INPUT_ERROR))? {
            let fields = field.as_object().ok_or_else(|| Error::invalid(INPUT_ERROR))?;
            if fields.len() != 2 || field["value"].as_str().is_none() {
                return Err(Error::invalid(INPUT_ERROR));
            }
            let id = field["fieldId"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| Error::invalid(INPUT_ERROR))?;
            if !seen.insert(id) {
                return Err(Error::invalid("Each custom field may only appear once"));
            }
        }
    }
    config::date(body["startDate"].as_str())?;
    if let Some(value) = body["dueDate"].as_str() {
        body["dueDate"] = json!(config::due_date(value)?);
    }
    if text(args, "file").is_none() {
        if let Some(value) = assignee(api, body["userId"].as_str()).await? {
            body["userId"] = json!(value);
        }
    }
    let project = api.config.required(args, "project")?.to_owned();
    output::project(
        &api.request(Method::POST, &["task", &project], &[], Some(body)).await?,
        &[TASK, &["description"]].concat(),
    )
}

async fn task_update(args: &ArgMatches, api: &mut Api) -> Result<Value> {
    let names = [
        "title",
        "description",
        "description-file",
        "status",
        "priority",
        "assignee",
        "unassign",
        "due",
        "clear-due",
    ];
    let selected: Vec<_> = names
        .into_iter()
        .filter(|key| args.value_source(key) == Some(clap::parser::ValueSource::CommandLine))
        .collect();
    if selected.len() != 1 {
        return Err(Error::invalid("Update requires exactly one field option"));
    }
    let (resource, body) = match selected[0] {
        "title" => ("title", json!({"title":api.config.required(args,"title")?})),
        "status" => ("status", json!({"status":api.config.required(args,"status")?})),
        "priority" => (
            "priority",
            json!({"priority":priority(text(args,"priority").unwrap_or(""))?}),
        ),
        "description" | "description-file" => (
            "description",
            json!({"description":config::input(text(args,"description"),text(args,"description-file")).await?.unwrap_or_default()}),
        ),
        "assignee" => (
            "assignee",
            json!({"userId":assignee(api,Some(api.config.required(args,"assignee")?.to_owned()).as_deref()).await?}),
        ),
        "unassign" => ("assignee", json!({"userId":null})),
        "due" => (
            "due-date",
            json!({"dueDate":config::due_date(text(args,"due").unwrap_or(""))?}),
        ),
        _ => ("due-date", json!({})),
    };
    output::project(
        &api.request(
            Method::PUT,
            &["task", resource, text(args, "id").unwrap_or("")],
            &[],
            Some(body),
        )
        .await?,
        &[TASK, &["description"]].concat(),
    )
}
