# Question: a `String` copied from a temporary view of a local whose last
# use is inside the return expression — `return String(head.rstrip("/"))` —
# and the self-assignment `head = String(head.rstrip("/"))`. Mojo accepts
# both (the right-hand side is evaluated before the local is destroyed or
# overwritten) and prints `/usr` twice.
#
# Mojito today: the checker accepts the program, but the VM reports
# `use after Pointer deallocation` — the local is destroyed at its last use
# (inside the view call) before the conversion copies the viewed bytes. The
# two-step spelling (`var stripped = String(head.rstrip("/")); return
# stripped^` / `head = stripped^`) runs; `std/os/path/path.mojo` uses it.
#
# On the fix: promote this file to an `assets/ok` fixture (parity +
# scalar manifest rows, exe ratchet bump), add a `cases.tsv` `run` row,
# delete the two-step workaround comments in `std/os/path/path.mojo`, and
# delete the "temporary view copied at a local's last use" bullet from the
# temporary-views task in `docs/roadmap.md`.
def strip_return(fspath: String) -> String:
    var head = String(fspath[byte=:5])
    return String(head.rstrip("/"))


def strip_assign(fspath: String) -> String:
    var head = String(fspath[byte=:5])
    head = String(head.rstrip("/"))
    return head


def main():
    print(strip_return(String("/usr/lib")))
    print(strip_assign(String("/usr/lib")))
