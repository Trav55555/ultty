-- A live list of Ultty's panes, refreshed every 2 seconds from
-- `ulttyctl --json panes`. Needs leave to run ulttyctl:
--
--     ulttui-lua --exec ulttyctl examples/lua/panes.luau
--     ulttyctl lua open examples/lua/panes.luau --exec ulttyctl
--
-- Keys: up / down (or k / j) move the selection, g refreshes now, q quits.

local function poll()
	return run { id = "panes", argv = { "ulttyctl", "--json", "panes" } }
end

-- One list row per pane: id, tab, program, status and working directory.
local function describe(pane)
	local program = pane.foreground or (pane.title ~= "" and pane.title) or "?"
	local status = "-"
	if pane.status then
		status = pane.status.state
		if pane.status.label and pane.status.label ~= "" then
			status ..= ": " .. pane.status.label
		end
	end
	local marker = pane.focused and "*" or " "
	return string.format(
		"%s pane %-3d tab %-2d %-12s %-18s %s",
		marker,
		pane.id,
		pane.tab_index + 1,
		program,
		status,
		pane.cwd or ""
	)
end

local function clamp(i, n)
	return math.max(1, math.min(i, math.max(n, 1)))
end

return app {
	init = function(prev)
		local state = prev or { panes = {}, selected = 1, error = nil, polls = 0 }
		return state, poll()
	end,

	every = { refresh = 2000 },

	update = function(state, msg)
		if msg.type == "tick" then
			return state, poll()
		elseif msg.type == "done" and msg.id == "panes" then
			state.polls += 1
			if msg.status ~= 0 then
				state.error = msg.error or msg.stderr
				return state
			end
			local reply = json.decode(msg.stdout)
			state.panes = reply.panes or {}
			state.error = nil
			state.selected = clamp(state.selected, #state.panes)
			return state
		elseif msg.type == "key" then
			local key = msg.key
			if key == "down" or key == "j" then
				state.selected = clamp(state.selected + 1, #state.panes)
			elseif key == "up" or key == "k" then
				state.selected = clamp(state.selected - 1, #state.panes)
			elseif key == "g" then
				return state, poll()
			elseif key == "q" then
				return state, quit()
			end
		end
		return state
	end,

	view = function(state)
		local rows = {}
		for _, pane in state.panes do
			table.insert(rows, describe(pane))
		end
		local selected = state.panes[state.selected]
		return column {
			gap = 1,
			padding = 1,
			text { "Ultty panes", bold = true, id = "title" },
			state.error and text { "ulttyctl failed: " .. state.error, fg = "red" },
			list { id = "panes", items = rows, selected = state.selected, grow = 1 },
			text {
				selected and ("selected: pane " .. selected.id .. " in " .. (selected.cwd or "?"))
					or "no panes",
				id = "selection",
			},
			text { "up/down move · g refresh · q quit · refreshes every 2 s", fg = "gray" },
		}
	end,
}
