# jed shell integration (sourced from ~/.zshrc)
[ -n "$JED_IPC" ] || return
# 633;E carries the command line, \ ; and control bytes escaped (\\ and \xNN).
_jed_esc() {
  local s=$1 c i
  _jed_e=$s
  case "$s" in *[\\\;[:cntrl:]]*) ;; *) return;; esac
  _jed_e=
  for ((i=1;i<=${#s};i++)); do
    c=${s[i]}
    case $c in
      \\) _jed_e+='\\';;
      ';'|[[:cntrl:]]) printf -v c '\\x%02x' "'$c"; _jed_e+=$c;;
      *) _jed_e+=$c;;
    esac
  done
}
_jed_preexec() {
  printf '\033]133;B\007'
  _jed_esc "$1"; printf '\033]633;E;%s\007' "$_jed_e"
  printf '\033]0;%s\007' "${1%% *}"
}
_jed_precmd() {
  local s=$? p=${PWD//\%/%25}
  printf '\033]133;D;%s\007' "$s"
  p=${p// /%20}; printf '\033]7;file://%s%s\007' "${HOST:-localhost}" "$p"
  # 133;A starts PS1, so every line of the prompt follows it (added again if a theme rebuilt PS1).
  case "$PS1" in *'133;A'*) ;; *) PS1=$'%{\033]133;A\007%}'"$PS1";; esac
  return $s
}
autoload -Uz add-zsh-hook; add-zsh-hook preexec _jed_preexec; add-zsh-hook precmd _jed_precmd
