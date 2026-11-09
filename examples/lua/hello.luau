-- A counter. Run it with `ulttui-lua examples/lua/hello.luau`, then edit
-- and save: the view changes and the count is kept.
--
-- Keys: + / - (or up / down) change the count, r resets, q quits.
-- Tab moves focus between the buttons; Enter presses the focused one.

local function change(state, by)
	return { count = state.count + by }
end

return app {
	init = function(prev)
		return prev or { count = 0 }
	end,

	update = function(state, msg)
		if msg.type ~= "key" then
			return state
		end
		local key = msg.key
		if key == "enter" then
			key = ({ inc = "+", dec = "-", reset = "r" })[msg.focused] or key
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
