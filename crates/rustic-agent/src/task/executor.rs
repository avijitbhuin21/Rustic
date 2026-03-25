use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, Role};
use crate::task::TaskStatus;
use crate::tools::{BuiltinTools, ToolContext, ToolExecutor};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Events emitted during task execution for real-time UI updates.
#[derive(Debug, Clone)]
pub enum TaskEvent {
    TextDelta { task_id: String, text: String },
    ToolUse { task_id: String, tool_name: String, tool_input: serde_json::Value },
    ToolResult { task_id: String, tool_use_id: String, output: String, is_error: bool },
    StatusChange { task_id: String, status: TaskStatus },
    MessageComplete { task_id: String, message: Message },
}

pub struct TaskExecutor {
    provider: Arc<dyn AiProvider>,
    tools: BuiltinTools,
    config: ProviderConfig,
}

impl TaskExecutor {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        config: ProviderConfig,
    ) -> Self {
        Self {
            provider,
            tools: BuiltinTools::new(),
            config,
        }
    }

    /// Run one turn of the agentic loop: send messages, handle tool calls, repeat until text-only response.
    pub async fn run_turn(
        &self,
        task_id: &str,
        messages: &mut Vec<Message>,
        context: &ToolContext,
        event_tx: &mpsc::UnboundedSender<TaskEvent>,
    ) -> Result<()> {
        let tool_defs = self.tools.definitions();

        loop {
            // Send to provider
            let response = self
                .provider
                .chat(messages.clone(), tool_defs.clone(), &self.config)
                .await?;

            // Build assistant message from response
            let assistant_msg = Message {
                role: Role::Assistant,
                content: response.content.clone(),
            };
            messages.push(assistant_msg.clone());

            // Emit text content
            for block in &response.content {
                if let ContentBlock::Text { text } = block {
                    let _ = event_tx.send(TaskEvent::TextDelta {
                        task_id: task_id.to_string(),
                        text: text.clone(),
                    });
                }
            }

            let _ = event_tx.send(TaskEvent::MessageComplete {
                task_id: task_id.to_string(),
                message: assistant_msg,
            });

            // Check if tool use is needed
            let tool_uses: Vec<_> = response
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();

            if tool_uses.is_empty() {
                // No tool calls — turn is complete
                break;
            }

            // Execute tools and build tool result message
            let mut tool_results = Vec::new();
            for (tool_id, tool_name, tool_input) in &tool_uses {
                let _ = event_tx.send(TaskEvent::ToolUse {
                    task_id: task_id.to_string(),
                    tool_name: tool_name.clone(),
                    tool_input: tool_input.clone(),
                });

                let result = self
                    .tools
                    .execute(tool_name, tool_input.clone(), context)
                    .await?;

                let _ = event_tx.send(TaskEvent::ToolResult {
                    task_id: task_id.to_string(),
                    tool_use_id: tool_id.clone(),
                    output: result.content.clone(),
                    is_error: result.is_error,
                });

                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: tool_id.clone(),
                    content: result.content,
                    is_error: result.is_error,
                });
            }

            // Add tool results as a user message (Claude expects this)
            messages.push(Message {
                role: Role::User,
                content: tool_results,
            });

            // Loop back for next provider call
        }

        Ok(())
    }
}
