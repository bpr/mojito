# A `String` copied from a temporary view of a local at the local's last
# use (a `return` expression, a self-assignment, and a reassignment whose
# call borrows the variable being overwritten): the view's source stays
# live until the copy or the call has read it.
def strip_return(fspath: String) -> String:
    var head = String(fspath[byte=:5])
    return String(head.rstrip("/"))


def strip_assign(fspath: String) -> String:
    var head = String(fspath[byte=:5])
    head = String(head.rstrip("/"))
    return head


def takes(v: StringSpan) -> String:
    var out = String("<")
    out += String(v)
    out += String(">")
    return out^


def reassign_through_slice() -> String:
    var head = String("/usr/lib")
    head = takes(head[byte=1:4])
    return head


def reassign_through_strip() -> String:
    var s = String("abc")
    s = takes(s.rstrip("c"))
    return s


def main():
    print(strip_return(String("/usr/lib")))
    print(strip_assign(String("/usr/lib")))
    print(reassign_through_slice())
    print(reassign_through_strip())
