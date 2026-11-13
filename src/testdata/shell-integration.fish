# jed shell integration (sourced from ~/.config/fish/config.fish)
set -q JED_IPC; or return
function _jed_preexec --on-event fish_preexec
  printf '\033]133;B\007'
  # 633;E: the command line with \ ; and newlines escaped (\\ and \xNN).
  set -l c (string replace -a '\\' '\\\\' -- $argv[1] | string replace -a ';' '\\x3b' | string join '\\x0a')
  printf '\033]633;E;%s\007' $c
  printf '\033]0;%s\007' (string split -m1 ' ' $argv)[1]
end
function _jed_precmd --on-event fish_postexec
  printf '\033]133;D;%s\007' $status
  printf '\033]7;file://%s%s\007' (hostname) (string escape --style=url -- $PWD)
end
# 133;A before the whole prompt: wrap fish_prompt (once), handing it the status it was called with.
function _jed_status; return $argv[1]; end
if not functions -q _jed_fish_prompt
  functions -c fish_prompt _jed_fish_prompt
  function fish_prompt
    set -l last $status
    printf '\033]133;A\007'
    _jed_status $last
    _jed_fish_prompt
  end
end
