# A call whose argument is a temporary plain-origin view of `s`
# (`StringSpan(s)` carries `origin_of(s)`, not an owned interior), assigned
# straight back to `s`. The pinned Mojo accepts it and prints `abc`; Mojito
# keeps the temporary's loan live through the store and rejects it as a
# conflict with that loan.
def takes(v: StringSpan) -> String:
    return String(v)


def main():
    var s = String("abc")
    s = takes(StringSpan(s))
    print(s)
