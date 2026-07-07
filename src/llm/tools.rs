use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    StringEnum(Vec<String>),
    StringArray,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolParam {
    pub name: String,
    pub description: String,
    pub param_type: ParamType,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ToolParam>,
}

impl ToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: Vec::new(),
        }
    }

    pub fn string_param(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        self.parameters.push(ToolParam {
            name: name.into(),
            description: description.into(),
            param_type: ParamType::String,
            required,
        });
        self
    }

    pub fn integer_param(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        self.parameters.push(ToolParam {
            name: name.into(),
            description: description.into(),
            param_type: ParamType::Integer,
            required,
        });
        self
    }

    pub fn enum_param(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        options: &[&str],
        required: bool,
    ) -> Self {
        self.parameters.push(ToolParam {
            name: name.into(),
            description: description.into(),
            param_type: ParamType::StringEnum(
                options.iter().map(|value| (*value).to_string()).collect(),
            ),
            required,
        });
        self
    }

    pub fn string_array_param(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        self.parameters.push(ToolParam {
            name: name.into(),
            description: description.into(),
            param_type: ParamType::StringArray,
            required,
        });
        self
    }

    pub fn to_openai_json(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for param in &self.parameters {
            let schema = match &param.param_type {
                ParamType::String => json!({
                    "type": "string",
                    "description": param.description,
                }),
                ParamType::Integer => json!({
                    "type": "integer",
                    "description": param.description,
                }),
                ParamType::StringEnum(options) => json!({
                    "type": "string",
                    "enum": options,
                    "description": param.description,
                }),
                ParamType::StringArray => json!({
                    "type": "array",
                    "items": { "type": "string" },
                    "description": param.description,
                }),
            };
            properties.insert(param.name.clone(), schema);
            if param.required {
                required.push(Value::String(param.name.clone()));
            }
        }

        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                }
            }
        })
    }
}

pub trait LlmTool: Send + Sync {
    fn definition(&self) -> &ToolDefinition;
    fn invoke(&self, arguments: &Value) -> Result<String, String>;
    fn is_terminal(&self) -> bool {
        false
    }
}

pub struct LlmToolSet {
    tools: Vec<Box<dyn LlmTool>>,
}

impl LlmToolSet {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register<T: LlmTool + 'static>(mut self, tool: T) -> Self {
        self.tools.push(Box::new(tool));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn definitions(&self) -> Vec<&ToolDefinition> {
        self.tools.iter().map(|tool| tool.definition()).collect()
    }

    pub fn to_openai_json(&self) -> Value {
        Value::Array(
            self.tools
                .iter()
                .map(|tool| tool.definition().to_openai_json())
                .collect(),
        )
    }

    pub fn invoke(&self, name: &str, arguments: &Value) -> Result<ToolInvocationResult, String> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.definition().name == name)
            .ok_or_else(|| format!("unsupported tool: {name}"))?;

        let output = tool.invoke(arguments)?;
        Ok(ToolInvocationResult {
            output,
            is_terminal: tool.is_terminal(),
        })
    }
}

impl Default for LlmToolSet {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocationResult {
    pub output: String,
    pub is_terminal: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool {
        definition: ToolDefinition,
    }

    impl EchoTool {
        fn new() -> Self {
            Self {
                definition: ToolDefinition::new("echo", "Echoes input").string_param(
                    "message",
                    "Text to echo",
                    true,
                ),
            }
        }
    }

    impl LlmTool for EchoTool {
        fn definition(&self) -> &ToolDefinition {
            &self.definition
        }

        fn invoke(&self, arguments: &Value) -> Result<String, String> {
            Ok(arguments["message"]
                .as_str()
                .unwrap_or_default()
                .to_string())
        }
    }

    #[test]
    fn tool_definition_serializes_to_openai_shape() {
        let tool = ToolDefinition::new("detect_language", "Detect language")
            .string_param("path", "Path", true)
            .enum_param("scope", "Scope", &["file", "project"], true);

        let json = tool.to_openai_json();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "detect_language");
        assert_eq!(
            json["function"]["parameters"]["required"],
            json!(["path", "scope"])
        );
    }

    #[test]
    fn tool_set_registers_and_invokes_tools() {
        let tools = LlmToolSet::new().register(EchoTool::new());
        let result = tools
            .invoke("echo", &json!({ "message": "hello" }))
            .expect("invoke");

        assert_eq!(result.output, "hello");
        assert!(!result.is_terminal);
        assert_eq!(tools.len(), 1);
    }
}
