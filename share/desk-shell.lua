-- A desk box's shell slot (#321): the user's own shell, as a desk-shells/ profile finds it, run in
-- the box's slot (share/shell.sh) instead of by the user's config. desk.lua calls take(cmd) for each
-- command the user's config starts, before its drop table (true: the slot runs it, so the config
-- does not), and after_start() once the user's config has run (the guess from OMABOX_DESK_SHELL when
-- nothing was taken). Step 0's placeholder: takes nothing, starts nothing (step 4 fills it).
return {
  take = function(cmd)
    return false
  end,
  after_start = function() end,
}
