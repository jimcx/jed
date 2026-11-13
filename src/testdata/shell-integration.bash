# jed shell integration (sourced from ~/.bashrc)
[ -n "$JED_IPC" ] || return
# 633;E carries the command line, \ ; and control bytes escaped (\\ and \xNN).
_jed_esc() {
  local s=$1 c i
  _jed_e=$s
  case "$s" in *[\\\;[:cntrl:]]*) ;; *) return;; esac
  _jed_e=
  for ((i=0;i<${#s};i++)); do
    c=${s:i:1}
    case $c in
      \\) _jed_e+='\\';;
      ';'|[[:cntrl:]]) printf -v c '\\x%02x' "'$c"; _jed_e+=$c;;
      *) _jed_e+=$c;;
    esac
  done
}
_jed_preexec() {
  case "$1" in PROMPT_COMMAND=*|_jed_precmd|_jed_mark) return;; esac
  local pc p rest nl=$'\n'
  for pc in "${PROMPT_COMMAND[@]}"; do
    [ "$1" = "$pc" ] && return
    # Each `;`- or newline-separated piece, trimmed (no word splitting, so no globbing).
    rest="${pc//$nl/;};"
    while [ -n "$rest" ]; do
      p=${rest%%;*}; rest=${rest#*;}
      p=${p#"${p%%[![:space:]]*}"}; p=${p%"${p##*[![:space:]]}"}
      [ "$1" = "$p" ] && return
    done
  done
  printf '\033]133;B\007'
  _jed_esc "$1"; printf '\033]633;E;%s\007' "$_jed_e"
  printf '\033]0;%s\007' "${1%% *}"
}
_jed_precmd() {
  local s=$? p=${PWD//\%/%25}
  printf '\033]133;D;%s\007' "$s"
  p=${p// /%20}; printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$p"
  return $s
}
# 133;A starts PS1, so every line of the prompt follows it; \[ \] keep readline's width right.
_jed_mark() { local s=$?; case "$PS1" in *'133;A'*) ;; *) PS1='\[\e]133;A\a\]'"$PS1";; esac; return $s; }
# _jed_precmd runs first, so the user's own PROMPT_COMMAND (a string, or an array in bash 5.1+)
# sees the real $?; _jed_mark last, after any of it that rebuilds PS1 (`__git_ps1`, themes).
case "$(declare -p PROMPT_COMMAND 2>/dev/null)" in
  *_jed_precmd*) ;;
  "declare -a"*) PROMPT_COMMAND=(_jed_precmd "${PROMPT_COMMAND[@]}" _jed_mark);;
  *) PROMPT_COMMAND="_jed_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"$'\n'_jed_mark;;
esac
trap '_jed_preexec "$BASH_COMMAND"' DEBUG
