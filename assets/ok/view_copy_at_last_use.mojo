# A `String` copied from a temporary view of a local at the local's last
# use (a `return` expression, and a copy bound to a fresh name before the
# local is reassigned): the view's source stays live until the copy or the
# call has read it. Assigning such a call straight back to the viewed local
# (`head = takes(head[byte=1:4])`) is rejected by both compilers (see
# `assets/ownership_error/assign_call_over_viewed_local.mojo`).
def strip_return(fspath: String) -> String:
    var head = String(fspath[byte=:5])
    return String(head.rstrip("/"))


def strip_assign(fspath: String) -> String:
    var head = String(fspath[byte=:5])
    var stripped = String(head.rstrip("/"))
    head = stripped^
    return head


def takes(v: StringSpan) -> String:
    var out = String("<")
    out += String(v)
    out += String(">")
    return out^


def through_slice() -> String:
    var head = String("/usr/lib")
    var taken = takes(head[byte=1:4])
    head = taken^
    return head


def through_strip() -> String:
    var s = String("abc")
    return takes(s.rstrip("c"))


def main():
    print(strip_return(String("/usr/lib")))
    print(strip_assign(String("/usr/lib")))
    print(through_slice())
    print(through_strip())
