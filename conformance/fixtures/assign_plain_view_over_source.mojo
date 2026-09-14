# A call whose argument is a temporary plain-origin view of `s`
# (`StringSpan(s)` carries `origin_of(s)`, not an owned interior), assigned
# straight back to `s`. The argument is not an owned interior of `s`, and the
# temporary's loan ends when the call returns, before the store, so both the
# pinned Mojo and Mojito accept it and print `abc`.
def takes(v: StringSpan) -> String:
    return String(v)


def main():
    var s = String("abc")
    s = takes(StringSpan(s))
    print(s)
