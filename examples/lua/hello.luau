-- A counter. Run it with `ulttui-lua examples/lua/hello.luau`, then edit
-- and save: the view changes and the count is kept.
--
-- Keys: + / - (or up / down) change the count, r resets, q quits.
-- Tab moves focus between the buttons; Enter presses the focused one.

local function change(state, by)
	return { count = state.count + by }
end

return app {
	actions = { inc = "activate", dec = "activate", reset = "activate" },
	init = function(prev)
		return prev or { count = 0 }
	end,

	update = function(state, msg)
		local targets = { inc = "+", dec = "-", reset = "r" }
		local key
		if msg.type == "action" and msg.action == "activate" then
			key = targets[msg.id]
		elseif msg.type == "key" then
			key = msg.key == "enter" and (targets[msg.focused] or msg.key) or msg.key
		else
			return state
		end
		if key == "+" or key == "=" or key == "up" then
			return change(state, 1)
		elseif key == "-" or key == "down" then
			return change(state, -1)
		elseif key == "r" then
			return { count = 0 }
		elseif key == "q" then
			return state, quit()
		end
		return state
	end,

	view = function(state)
		local colour = state.count > 0 and "green" or state.count < 0 and "red" or nil
		return column {
			gap = 1,
			padding = 1,
			text { "Hello from Luau", bold = true },
			text { "Count: " .. state.count, id = "count", fg = colour },
			row {
				gap = 2,
				button { id = "dec", label = "-1" },
				button { id = "inc", label = "+1", variant = "primary" },
				button { id = "reset", label = "Reset" },
			},
			text { "+/- · Tab · Enter · r reset · q quit", fg = "gray" },
		}
	end,
}
