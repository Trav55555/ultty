# Ultty shell integration for zsh.
#
# Marks each prompt, command line, command output and exit status with OSC
# 133, so `ulttyctl last-command`, `ulttyctl wait-for --command-done` and the
# `command_started` / `command_finished` events work. Other terminals that
# understand OSC 133 use the same marks; the rest ignore them.
#
# Source it at the end of ~/.zshrc (after any prompt theme):
#   source /usr/share/ultty/shell-integration/ultty.zsh

[[ -o interactive ]] || return 0
(( ${+__ultty_integration} )) && return 0
typeset -g __ultty_integration=1
typeset -g __ultty_running=

# D: the last command finished. First among the precmd hooks, so $? is
# still the command's status.
__ultty_command_done() {
    local ret=$?
    if [[ -n $__ultty_running ]]; then
        printf '\e]133;D;%s\a' "$ret"
        __ultty_running=
    fi
    return $ret
}

# A and B around the prompt. Last among the precmd hooks, because prompt
# themes rebuild PS1 in theirs.
__ultty_mark_prompt() {
    [[ $PS1 == *$'\e]133;A'* ]] || PS1=$'%{\e]133;A\a%}'"$PS1"
    [[ $PS1 == *$'\e]133;B'* ]] || PS1="$PS1"$'%{\e]133;B\a%}'
}

# C: output starts. zsh runs preexec after the command line is accepted.
__ultty_command_start() {
    printf '\e]133;C\a'
    __ultty_running=1
}

# When the output's last line had no newline, zsh prints PROMPT_EOL_MARK
# (an inverse %) after it before running precmd. Send D from the mark
# itself, so the command ends where its output does and the mark is not
# taken for output. %? is the command's status. The D sent by precmd then
# finds no command running and marks nothing.
PROMPT_EOL_MARK=$'%{\e]133;D;%?\a%}'"${PROMPT_EOL_MARK-%B%S%#%s%b}"

typeset -ga precmd_functions preexec_functions
precmd_functions=(__ultty_command_done ${precmd_functions:#__ultty_*} __ultty_mark_prompt)
preexec_functions+=(__ultty_command_start)
