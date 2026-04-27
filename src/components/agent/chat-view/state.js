// Module-level Maps shared between createChatView and the standalone render
// functions. Pulled out of chat-view.js so the renderers can live in their
// own files without cross-importing the giant main module.
//
// Keys:
//   expandedState: opaque string keys ("thinking-{msgIdx}", "tool-{tool_use_id}",
//                  "group-{firstToolUseId}") → boolean (open/closed)
//   thinkingStartTimes: thinkingKey → ms timestamp when the thinking block
//                       first started streaming
//   thinkingWordCounts: thinkingKey → running word count for the live label

export const expandedState = new Map();
export const thinkingStartTimes = new Map();
export const thinkingWordCounts = new Map();
