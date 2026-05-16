# analyze_cache_dumps.ps1
#
# Reads the .summary.json files written by RUSTIC_DUMP_PROMPTS and produces
# a complete cache-write diagnosis for a T2-style task run:
#
#   1. Per-turn table  — turn #, message count, cache_write, cache_read, notes
#   2. Condense events — turns where message count dropped (condense fired)
#   3. Changed-message detail — which existing messages had content changes
#      between consecutive turns (the primary cache-invalidation signal)
#   4. Root-cause summary — which mechanisms are active
#
# Usage:
#   .\docs\analyze_cache_dumps.ps1 -DumpDir "C:\tmp\rustic-dumps"
#   .\docs\analyze_cache_dumps.ps1 -DumpDir "C:\tmp\rustic-dumps" -TaskId "abc123"
#   .\docs\analyze_cache_dumps.ps1 -DumpDir "C:\tmp\rustic-dumps" -DetailTurns 10,11,50,51

param(
    [Parameter(Mandatory=$true)]
    [string]$DumpDir,

    # Optional: filter to a specific task by partial ID match
    [string]$TaskId = "",

    # Optional: print block-level diff for specific turn numbers (comma-separated)
    [int[]]$DetailTurns = @()
)

Set-StrictMode -Version 2
$ErrorActionPreference = "Stop"

# ── Load summary files ────────────────────────────────────────────────────────

$filter = if ($TaskId) { "*$TaskId*turn-*.summary.json" } else { "*-turn-*.summary.json" }
$files  = Get-ChildItem -Path $DumpDir -Filter $filter -ErrorAction SilentlyContinue |
          Sort-Object Name

if ($files.Count -eq 0) {
    Write-Host "No summary files found in '$DumpDir' matching '$filter'" -ForegroundColor Red
    Write-Host ""
    Write-Host "Make sure you:"
    Write-Host "  1. Set `$env:RUSTIC_DUMP_PROMPTS = 'C:\tmp\rustic-dumps' before launching Rustic"
    Write-Host "  2. Ran a task (summary files are written after every API call)"
    exit 1
}

Write-Host "Loaded $($files.Count) summary files from $DumpDir" -ForegroundColor Cyan
Write-Host ""

$turns = @()
foreach ($f in $files) {
    try {
        $data = Get-Content $f.FullName -Raw | ConvertFrom-Json
        $turns += $data
    } catch {
        Write-Host "  [WARN] Could not parse $($f.Name): $_" -ForegroundColor Yellow
    }
}

if ($turns.Count -eq 0) {
    Write-Host "All files failed to parse — nothing to analyse." -ForegroundColor Red
    exit 1
}

# ── Per-turn table ────────────────────────────────────────────────────────────

Write-Host "=== PER-TURN CACHE STATS ===" -ForegroundColor Cyan
$hdr = "{0,-5}  {1,-5}  {2,11}  {3,11}  {4,10}  {5,7}  {6}"
Write-Host ($hdr -f "Turn", "Msgs", "CacheWrite", "CacheRead", "InputToks", "OutToks", "Notes")
Write-Host ("-" * 100)

$prevTurn        = $null
$totalCacheWrite = 0
$totalCacheRead  = 0
$agingEventCount = 0
$condenseCount   = 0
$toolLoadCount   = 0
$syspromptChanges= 0

# Collect per-turn note arrays for the root-cause section
$turnNotes = @{}

foreach ($t in $turns) {
    $notes = [System.Collections.Generic.List[string]]::new()

    $cw  = if ($null -ne $t.cache_write_tokens) { [int]$t.cache_write_tokens } else { -1 }
    $cr  = if ($null -ne $t.cache_read_tokens)  { [int]$t.cache_read_tokens  } else { -1 }
    $inp = if ($null -ne $t.input_tokens)        { [int]$t.input_tokens       } else { -1 }
    $out = if ($null -ne $t.output_tokens)       { [int]$t.output_tokens      } else { -1 }

    if ($cw  -ge 0) { $totalCacheWrite += $cw }
    if ($cr  -ge 0) { $totalCacheRead  += $cr }

    if ($prevTurn) {
        # ── Condense: message count dropped ──────────────────────────────
        if ($t.message_count -lt $prevTurn.message_count) {
            $notes.Add("CONDENSE($($prevTurn.message_count)->$($t.message_count)msgs)")
            $condenseCount++
        }

        # ── Tool-search load: new tools in the visible pool ───────────────
        if ($t.tool_count -ne $prevTurn.tool_count) {
            $newTools = @($t.tool_names) | Where-Object { $_ -notin @($prevTurn.tool_names) }
            if ($newTools.Count -gt 0) {
                $notes.Add("TOOL_LOADED:[$($newTools -join ',')]")
                $toolLoadCount++
            }
        }

        # ── System-prompt change ─────────────────────────────────────────
        if ($t.system_prompt_hash -ne $prevTurn.system_prompt_hash) {
            $lenDelta = [int]$t.system_prompt_len - [int]$prevTurn.system_prompt_len
            $notes.Add("SYS_PROMPT_CHANGED(delta=${lenDelta}B)")
            $syspromptChanges++
        }

        # ── Changed existing messages (not just new ones appended) ────────
        $minMsgs = [Math]::Min($t.messages.Count, $prevTurn.messages.Count)
        $changedMsgs = [System.Collections.Generic.List[string]]::new()

        for ($i = 0; $i -lt $minMsgs; $i++) {
            $pm = $prevTurn.messages[$i]
            $cm = $t.messages[$i]

            if ($pm.total_len -ne $cm.total_len) {
                $role  = $cm.role
                $delta = [int]$cm.total_len - [int]$pm.total_len
                $changedMsgs.Add("msg[$i]($role) ${delta}B")

                # Detect aging: a tool_result shrank
                for ($k = 0; $k -lt [Math]::Min($pm.blocks.Count, $cm.blocks.Count); $k++) {
                    $pb = $pm.blocks[$k]; $cb = $cm.blocks[$k]
                    if ($pb.kind -eq "tool_result" -and $cb.kind -eq "tool_result") {
                        if ($cb.len -lt $pb.len) { $agingEventCount++ }
                    }
                }
            }
        }

        if ($changedMsgs.Count -gt 0) {
            $notes.Add("MSGS_CHANGED:[$($changedMsgs -join '|')]")
        }
    }

    $noteStr = $notes -join "  "
    $cwDisp  = if ($cw  -ge 0) { $cw.ToString("N0")  } else { "?" }
    $crDisp  = if ($cr  -ge 0) { $cr.ToString("N0")  } else { "?" }
    $inpDisp = if ($inp -ge 0) { $inp.ToString("N0") } else { "?" }
    $outDisp = if ($out -ge 0) { $out.ToString("N0") } else { "?" }

    $color = "White"
    if     ($noteStr -match "CONDENSE")        { $color = "Red"     }
    elseif ($noteStr -match "MSGS_CHANGED")    { $color = "Yellow"  }
    elseif ($noteStr -match "TOOL_LOADED")     { $color = "Cyan"    }
    elseif ($noteStr -match "SYS_PROMPT")      { $color = "Magenta" }
    elseif ($cw -gt 15000)                     { $color = "Yellow"  }

    Write-Host ($hdr -f $t.turn, $t.message_count, $cwDisp, $crDisp, $inpDisp, $outDisp, $noteStr) `
        -ForegroundColor $color

    $turnNotes[[int]$t.turn] = $notes
    $prevTurn = $t
}

Write-Host ("-" * 100)
$avgCW = if ($turns.Count -gt 0) { [Math]::Round($totalCacheWrite / $turns.Count, 0) } else { 0 }
Write-Host ("TOTALS  cache_write={0:N0}  cache_read={1:N0}  avg_write/turn={2:N0}" `
    -f $totalCacheWrite, $totalCacheRead, $avgCW) -ForegroundColor Cyan
Write-Host ""

# ── Condense events detail ────────────────────────────────────────────────────

if ($condenseCount -gt 0) {
    Write-Host "=== CONDENSE EVENTS ($condenseCount) ===" -ForegroundColor Red
    for ($i = 1; $i -lt $turns.Count; $i++) {
        $prev = $turns[$i-1]; $cur = $turns[$i]
        if ($cur.message_count -lt $prev.message_count) {
            $cw = if ($null -ne $cur.cache_write_tokens) { [int]$cur.cache_write_tokens } else { "?" }
            Write-Host ("  After turn {0}: {1}→{2} msgs  cache_write={3:N0}" `
                -f $cur.turn, $prev.message_count, $cur.message_count, $cw) -ForegroundColor Red

            # Show first message fingerprint after condense (position 1 = the summary)
            if ($cur.messages.Count -gt 1) {
                $summaryMsg = $cur.messages[1]
                $summaryLen = $summaryMsg.total_len
                Write-Host ("    messages[1] role={0}  total_len={1:N0}  (this is the condense summary + F4 block)" `
                    -f $summaryMsg.role, $summaryLen) -ForegroundColor DarkRed
            }
        }
    }
    Write-Host ""
}

# ── Changed-message block-level detail ───────────────────────────────────────

Write-Host "=== CHANGED-MESSAGE DETAIL ===" -ForegroundColor Cyan
Write-Host "Turns where existing messages changed content (not just new appends):"
Write-Host "(These are the cache-invalidation events — the bytes Anthropic cached no longer match)"
Write-Host ""

$anyChanges = $false

for ($i = 1; $i -lt $turns.Count; $i++) {
    $prev = $turns[$i-1]; $cur = $turns[$i]
    $minMsgs = [Math]::Min($cur.messages.Count, $prev.messages.Count)
    $changedLines = [System.Collections.Generic.List[string]]::new()

    for ($j = 0; $j -lt $minMsgs; $j++) {
        $pm = $prev.messages[$j]; $cm = $cur.messages[$j]
        if ($pm.total_len -eq $cm.total_len) { continue }  # quick skip

        $minBlocks = [Math]::Min($pm.blocks.Count, $cm.blocks.Count)
        for ($k = 0; $k -lt $minBlocks; $k++) {
            $pb = $pm.blocks[$k]; $cb = $cm.blocks[$k]
            $prevLen = if ($null -ne $pb.len) { $pb.len } elseif ($null -ne $pb.input_len) { $pb.input_len } else { 0 }
            $curLen  = if ($null -ne $cb.len) { $cb.len } elseif ($null -ne $cb.input_len) { $cb.input_len } else { 0 }
            if ($pb.hash -ne $cb.hash) {
                $delta = [int]$curLen - [int]$prevLen
                $why   = if ($cb.kind -eq "tool_result" -and $delta -lt 0) {
                    " ← AGING SHRINK"
                } elseif ($j -eq 1 -and $k -eq 0) {
                    " ← CONDENSE SUMMARY SWAP"
                } else { "" }
                $changedLines.Add(
                    "    msg[$j]($($cm.role)) block[$k]($($cb.kind))  " +
                    "${prevLen}→${curLen}B (delta=${delta})${why}"
                )
                # Show preview of what changed
                if ($null -ne $cb.preview -and $cb.preview.Length -gt 0) {
                    $p = ($cb.preview -replace "`n"," ").Substring(0, [Math]::Min(80, $cb.preview.Length))
                    $changedLines.Add("      preview: $p")
                }
            }
        }
    }

    if ($changedLines.Count -gt 0) {
        $cw = if ($null -ne $cur.cache_write_tokens) { [int]$cur.cache_write_tokens } else { "?" }
        Write-Host "Turn $($cur.turn)  cache_write=$($cw.ToString("N0")):" -ForegroundColor Yellow
        foreach ($line in $changedLines) { Write-Host $line }
        Write-Host ""
        $anyChanges = $true
    }
}

if (-not $anyChanges) {
    Write-Host "  None detected. Existing messages are stable between turns." -ForegroundColor Green
    Write-Host "  Cache writes are only from newly appended messages (expected behaviour)." -ForegroundColor Green
    Write-Host ""
}

# ── Optional: per-block detail for specific turns ─────────────────────────────

if ($DetailTurns.Count -gt 0) {
    Write-Host "=== DETAILED BLOCK DUMP FOR REQUESTED TURNS ===" -ForegroundColor Magenta
    foreach ($tn in $DetailTurns) {
        $t = $turns | Where-Object { [int]$_.turn -eq $tn } | Select-Object -First 1
        if (-not $t) { Write-Host "  Turn $tn not found." -ForegroundColor Yellow; continue }
        Write-Host "Turn $tn  msgs=$($t.message_count)  cache_write=$($t.cache_write_tokens)" -ForegroundColor Magenta
        for ($j = 0; $j -lt $t.messages.Count; $j++) {
            $m = $t.messages[$j]
            Write-Host "  msg[$j] $($m.role)  total_len=$($m.total_len)"
            for ($k = 0; $k -lt $m.blocks.Count; $k++) {
                $b = $m.blocks[$k]
                $len = if ($null -ne $b.len) { $b.len } elseif ($null -ne $b.input_len) { $b.input_len } else { "?" }
                $preview = if ($null -ne $b.preview) { ($b.preview -replace "`n"," ").Substring(0,[Math]::Min(60,$b.preview.Length)) } else { "" }
                Write-Host ("    block[$k] $($b.kind)  len=$len  hash=$($b.hash)  $preview")
            }
        }
        Write-Host ""
    }
}

# ── Root-cause summary ────────────────────────────────────────────────────────

Write-Host "=== ROOT CAUSE SUMMARY ===" -ForegroundColor Cyan
Write-Host "Turns analysed : $($turns.Count)"
Write-Host "Avg cache_write: $($avgCW.ToString("N0")) tokens/turn"
Write-Host ""

$found = $false

if ($condenseCount -gt 0) {
    Write-Host "[CONDENSE — HIGH IMPACT]" -ForegroundColor Red
    Write-Host "  $condenseCount condense event(s) detected."
    Write-Host "  Each condense replaces the conversation middle with a new summary message."
    Write-Host "  Because the prefix changes, ALL tail messages must be re-cached — typically"
    Write-Host "  60K-120K tokens per event on T2-style workloads."
    Write-Host "  Fix: cap or remove the F4 preserved-reads block (see O1 in perf_findings_v2.md)."
    Write-Host ""
    $found = $true
}

if ($agingEventCount -gt 0) {
    Write-Host "[AGING SHRINK — ONGOING]" -ForegroundColor Yellow
    Write-Host "  $agingEventCount tool_result shrink events detected across turns."
    Write-Host "  AGE_KEEP_FULL=6 means one old result is newly shrunk every turn."
    Write-Host "  The first time a result shrinks its bytes change -> cache miss from that"
    Write-Host "  position forward -> re-write of everything up to the last cache marker."
    Write-Host "  Fix: remove aging for the main agent (sub-agents already skip it),"
    Write-Host "  or raise AGE_KEEP_FULL so fewer results flip per turn."
    Write-Host ""
    $found = $true
}

if ($toolLoadCount -gt 0) {
    Write-Host "[TOOL_SEARCH LOADS — ONE-TIME PER LOAD]" -ForegroundColor Cyan
    Write-Host "  $toolLoadCount turn(s) where a new deferred tool was added to the visible pool."
    Write-Host "  The last tool in the list carries cache_control; a new last tool invalidates"
    Write-Host "  the entire tool-definitions block (~5K tokens) for that turn."
    Write-Host ""
    $found = $true
}

if ($syspromptChanges -gt 0) {
    Write-Host "[SYSTEM PROMPT CHANGED — UNEXPECTED]" -ForegroundColor Magenta
    Write-Host "  $syspromptChanges turn(s) where the system prompt hash changed."
    Write-Host "  The system prompt should be stable within a task. Possible causes:"
    Write-Host "    - Skills/workflows/rules file changed on disk mid-task"
    Write-Host "    - Plan-mode or goal-mode toggled"
    Write-Host "    - MCP tools connected/disconnected (appended to system prompt)"
    Write-Host "  This invalidates the system prompt cache breakpoint (~8K tokens) AND"
    Write-Host "  everything after it in the request."
    Write-Host ""
    $found = $true
}

if (-not $found) {
    Write-Host "No cache-invalidating patterns detected from fingerprints." -ForegroundColor Green
    Write-Host "The per-turn writes appear to be only from newly appended messages."
    Write-Host ""
    Write-Host "If cache_write is still high, run with RUSTIC_DUMP_PROMPTS_FULL=1 and"
    Write-Host "byte-diff a high-write turn pair:"
    Write-Host "  Compare-Object (Get-Content '<task>-turn-0010.full.json') \"
    Write-Host "                 (Get-Content '<task>-turn-0011.full.json')"
}
