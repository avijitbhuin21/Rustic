# Model Switching and Thinking API Fix

## Issues

### Issue 1: Model Switching with Thinking Enabled
When switching between Claude models mid-chat (e.g., Sonnet 4.6 → Opus 4.8), the next API call would fail with a 400 error about incompatible thinking configurations.

### Issue 2: Thinking "Off" Still Enables Thinking
Even when the user explicitly set thinking to "Off", Opus 4.8 would still receive a 400 error about adaptive thinking not being supported.

## Root Causes

### Cause 1: Incorrect Model Detection
The `supports_adaptive_thinking()` function was using overly broad pattern matching that didn't account for the differences between Claude model generations:

- **Opus 4.8 and 4.7**: ONLY support adaptive thinking (`thinking: {type: "adaptive"}`). Manual budget_tokens returns 400 error.
- **Opus 4.6 and Sonnet 4.6**: Support both adaptive (recommended) and manual budget_tokens (deprecated).
- **Older models (4.5 and below)**: Only support manual budget_tokens.

The code was treating all 4.x models the same way, which caused the wrong thinking configuration to be sent.

### Cause 2: "Off" Defaulting to 10k
When the user set thinking to "Off", the frontend sent `null` to the backend. The backend then defaulted to 10,000 tokens for Claude models, effectively enabling thinking even when the user explicitly turned it off.

## Fixes

### Fix 1: Explicit Model Version Matching
Updated `crates/rustic-agent/src/provider/claude.rs` to explicitly list which models support adaptive thinking:

```rust
fn supports_adaptive_thinking(model: &str) -> bool {
    // Opus 4.7+ and Mythos ONLY support adaptive thinking
    // Opus/Sonnet 4.6 support both but adaptive is recommended
    // Older models only support manual budget_tokens
    model.contains("opus-4-7")
        || model.contains("opus-4-8") 
        || model.contains("opus-4-9")
        || model.contains("opus-4-6")
        || model.contains("sonnet-4-6")
        || model.contains("mythos")
}
```

This ensures:
- Opus 4.8 uses adaptive thinking (required)
- Opus 4.7 uses adaptive thinking (required)
- Opus/Sonnet 4.6 use adaptive thinking (recommended over deprecated manual)
- Future 4.9 models are supported
- Older models use manual budget_tokens

### Fix 2: Explicit Zero for "Off"
Updated `src/state/agent.js` to explicitly send `0` when thinking is "Off":

```javascript
export function thinkingTierToBudget(tier) {
  switch (tier) {
    case 'off':    return 0;  // Explicitly disable thinking
    case 'low':    return 1024;
    case 'medium': return 4096;
    case 'high':   return 16384;
    case 'max':    return 32768;
    default:       return null;
  }
}
```

Now when thinking is "Off", the backend receives `0` and will not enable thinking.

## Testing

To test the fixes in dev mode:

1. **Build the updated code:**
   ```bash
   cargo build
   ```

2. **Run the app:**
   ```bash
   bun run tauri dev
   ```

3. **Test Scenario 1: Model Switching with Thinking**
   - Start a chat with **Opus 4.8** with thinking set to **High**
   - Send a message and verify it works
   - Switch to **Sonnet 4.6** 
   - Send another message (should work)
   - Switch back to **Opus 4.8**
   - Send another message (previously failed, should now work)

4. **Test Scenario 2: Thinking Off**
   - Set thinking to **Off**
   - Start a new chat with **Opus 4.8**
   - Send a message (previously failed, should now work with no thinking)

5. **Expected behavior:**
   - All messages should complete successfully
   - No 400 errors about thinking configurations
   - Model switches work seamlessly
   - "Off" truly disables thinking
   - Each model uses its appropriate thinking API format

## Technical Details

### Adaptive vs Manual Thinking

**Adaptive Thinking (Opus 4.6+):**
```json
{
  "thinking": {
    "type": "adaptive",
    "display": "summarized"
  },
  "output_config": {
    "effort": "high"
  }
}
```

**Manual Thinking (Older models):**
```json
{
  "thinking": {
    "type": "enabled",
    "budget_tokens": 16384
  }
}
```

### Thinking Disabled:
```json
{
  // thinking field omitted entirely
}
```

## References
- [Claude Opus 4.8 Documentation](https://platform.claude.com/docs/en/about-claude/models/whats-new-claude-4-8)
- [Effort Parameter Guide](https://platform.claude.com/docs/en/build-with-claude/effort)
- [Extended Thinking Guide](https://platform.claude.com/docs/en/build-with-claude/extended-thinking)

