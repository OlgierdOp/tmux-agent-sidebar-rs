# tmux agent sidebar. Load it from ~/.tmux.conf:
#   source-file /path/to/tmux-agent-sidebar-rs/agent-sidebar.tmux
set -gF @agent_sidebar_dir "#{d:current_file}"
# The binary is target/release/agent-sidebar: run `cargo build --release` first.

# Selection background (xterm-256 color). Pick one live with `c` in the sidebar.
# set -g @agent_sidebar_bg 23

# prefix + a   -> show/hide the sidebar (in every window, shared state)
bind-key a run-shell -b "#{@agent_sidebar_dir}/target/release/agent-sidebar toggle"
# prefix + Tab -> go to the agent that is waiting (red first, then green)
bind-key Tab run-shell -b "#{@agent_sidebar_dir}/target/release/agent-sidebar next"

# Ctrl+a (no prefix) -> go into the sidebar of this window, from any pane.
# Turns the sidebar on when it is off. A normal press starts no process.
bind-key -n C-a {
  if -F "#{@agent_sidebar_on}" {
    if -F "#{==:#{P:#{?#{@agent_sidebar},x,}},}" {
      run-shell -b "#{@agent_sidebar_dir}/target/release/agent-sidebar focus #{window_id}"
    } {
      run-shell -C "select-pane -t #{P:#{?#{@agent_sidebar},#{pane_id},}}"
    }
  } {
    run-shell -b "#{@agent_sidebar_dir}/target/release/agent-sidebar toggle"
  }
}

# On a window, session or pane switch:
# - a window without a sidebar (new, or from another session) gets one,
# - a window with a sidebar gets F12, so the sidebar redraws at once.
# tmux evaluates the conditions and `run-shell -C` runs a tmux command, so a
# normal switch starts no process.
set-hook -g session-window-changed[42] {
  if -F "#{@agent_sidebar_on}" {
    if -F "#{==:#{P:#{?#{@agent_sidebar},x,}},}" {
      run-shell -b "#{@agent_sidebar_dir}/target/release/agent-sidebar ensure #{window_id}"
    } {
      run-shell -C "send-keys -t #{P:#{?#{@agent_sidebar},#{pane_id},}} F12"
    }
  }
}
set-hook -g client-session-changed[42] {
  if -F "#{@agent_sidebar_on}" {
    if -F "#{==:#{P:#{?#{@agent_sidebar},x,}},}" {
      run-shell -b "#{@agent_sidebar_dir}/target/release/agent-sidebar ensure #{window_id}"
    } {
      run-shell -C "send-keys -t #{P:#{?#{@agent_sidebar},#{pane_id},}} F12"
    }
  }
}
set-hook -g after-select-pane[42] {
  if -F "#{&&:#{@agent_sidebar_on},#{!=:#{P:#{?#{@agent_sidebar},x,}},}}" {
    run-shell -C "send-keys -t #{P:#{?#{@agent_sidebar},#{pane_id},}} F12"
  }
}
set-hook -g after-new-window[42] {
  if -F "#{&&:#{@agent_sidebar_on},#{==:#{P:#{?#{@agent_sidebar},x,}},}}" {
    run-shell -b "#{@agent_sidebar_dir}/target/release/agent-sidebar ensure #{window_id}"
  }
}

# When a pane closes:
# - a window with only the sidebar left closes,
# - a sidebar that got the closed pane's space shrinks back before tmux draws,
# - the visible sidebars get F12, so the closed agent leaves the list at once.
# tmux evaluates these hooks in the current window, not in the changed one,
# so the formats loop over the windows (#{W:}, #{S:#{W:}}) and build the
# commands (only for a window with one sidebar). No process starts.
set -g @agent_sidebar_close "#{W:#{?#{&&:#{==:#{window_panes},1},#{@agent_sidebar}},kill-pane -t #{pane_id} ;,}}"
set -g @agent_sidebar_fit "#{S:#{W:#{?#{&&:#{!=:#{window_panes},1},#{&&:#{!=:#{window_zoomed_flag},1},#{&&:#{==:#{P:#{?#{@agent_sidebar},x,}},x},#{!=:#{P:#{?#{@agent_sidebar},#{pane_width},}},#{@agent_sidebar_width}}}}},resize-pane -t #{P:#{?#{@agent_sidebar},#{pane_id},}} -x #{@agent_sidebar_width} ;,}}}"
set -g @agent_sidebar_poke "#{S:#{W:#{?#{&&:#{window_active_clients},#{!=:#{window_panes},1}},#{P:#{?#{@agent_sidebar},send-keys -t #{pane_id} F12 ;,}},}}}"
set-hook -g window-layout-changed[42] {
  if -F "#{&&:#{@agent_sidebar_on},#{@agent_sidebar_width}}" {
    run-shell -C "#{E:@agent_sidebar_close} #{E:@agent_sidebar_fit}"
  }
}
set-hook -g pane-exited[42] {
  if -F "#{@agent_sidebar_on}" { run-shell -C "#{E:@agent_sidebar_poke}" }
}
set-hook -g after-kill-pane[42] {
  if -F "#{@agent_sidebar_on}" { run-shell -C "#{E:@agent_sidebar_poke}" }
}
set-hook -g window-unlinked[42] {
  if -F "#{@agent_sidebar_on}" { run-shell -C "#{E:@agent_sidebar_poke}" }
}

# tmux-resurrect: after a restore, start `claude --resume <id>` in the panes
# that ran an agent and close the old sidebar panes. Turn it off with:
#   set -g @agent_sidebar_resume off
set -gF @resurrect-hook-post-restore-all "#{@agent_sidebar_dir}/target/release/agent-sidebar resume"
