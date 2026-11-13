# Ultty shell integration for fish 3.4 or later.
#
# Marks each prompt, command line, command output and exit status with OSC
# 133, and reports the working directory with OSC 7 at every prompt. Other
# terminals that understand these use the same marks; the rest ignore them.
#
# Ultty loads it by itself in the shells it starts (`shell_integration` in the
# config), through a vendor_conf.d directory on XDG_DATA_DIRS. To load it by
# hand instead, copy it to ~/.config/fish/conf.d/.

# Ultty put its directory first on XDG_DATA_DIRS so fish would find this file.
# Take it out again so programs the shell starts do not inherit it.
if set -q ULTTY_SHELL_INTEGRATION_XDG_DIR
    set -gx XDG_DATA_DIRS (string join : -- (string match -v -- $ULTTY_SHELL_INTEGRATION_XDG_DIR (string split : -- $XDG_DATA_DIRS)))
    set -e ULTTY_SHELL_INTEGRATION_XDG_DIR
end

if status is-interactive; and not set -q __ultty_integration
    set -g __ultty_integration 1
    set -g __ultty_state none

    # OSC 7 and A, then B once the prompt text is out. A command that was
    # interrupted without fish_postexec firing still gets its D here.
    function __ultty_on_prompt --on-event fish_prompt
        if test "$__ultty_state" = running
            printf '\e]133;D\a'
        end
        set -g __ultty_state prompt
        printf '\e]7;file://%s%s\a' $hostname (string escape --style=url -- $PWD)
        printf '\e]133;A\a'
        # Wrap the prompt on the first prompt, after config.fish defined it.
        if not set -q __ultty_prompt_wrapped; and functions -q fish_prompt
            set -g __ultty_prompt_wrapped 1
            functions -c fish_prompt __ultty_user_prompt
            function fish_prompt
                __ultty_user_prompt
                printf '\e]133;B\a'
            end
        end
    end

    function __ultty_on_preexec --on-event fish_preexec
        set -g __ultty_state running
        printf '\e]133;C\a'
    end

    function __ultty_on_postexec --on-event fish_postexec
        set -l last_status $status
        set -g __ultty_state done
        printf '\e]133;D;%s\a' $last_status
    end
end
