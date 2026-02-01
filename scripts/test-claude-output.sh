#!/bin/bash
# Simulate Claude-like TUI output to test rendering

# Clear screen and move to top
printf '\033[2J\033[H'

# Draw a box like Claude does
printf '\033[38;5;208m'  # Orange color
printf '┌─────────────────────────────────────────┐\n'
printf '│                                         │\n'
printf '│   Welcome back!                         │\n'
printf '│                                         │\n'
printf '│   ███                                   │\n'
printf '│   █▀█  ← Robot mascot                   │\n'
printf '│   ███                                   │\n'
printf '│   █ █                                   │\n'
printf '│                                         │\n'
printf '└─────────────────────────────────────────┘\n'
printf '\033[0m'  # Reset

# Test cursor positioning like Claude does
printf '\n\033[38;5;245mTips for getting started\033[0m\n'
printf 'Run /init to create a CLAUDE.md file\n\n'

# Test the "Try" prompt
printf '\033[38;5;245m) Try "hello world"\033[0m\n'
printf '? for shortcuts\n'

# Keep terminal open
read -p "Press Enter to exit..."
