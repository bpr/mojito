# A named plain-origin view (`StringSpan(s)` borrows `s` itself, not its
# owned bytes) passed to a call assigned back to `s`. The pinned Mojo
# prints `abc`.
def takes(v: StringSpan) -> String:
    return String(v)

def main():
    var s = String("abc")
    var v = StringSpan(s)
    s = takes(v)
    print(s)
