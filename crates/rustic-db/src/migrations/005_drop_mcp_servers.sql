-- MCP servers moved from SQLite to per-scope JSON files
-- (user: app-data-dir/mcp.json, project: .mcp.json) to match Claude Code.
DROP TABLE IF EXISTS mcp_servers;
