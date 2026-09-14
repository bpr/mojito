# A temporary plain-origin view (`StringSpan(s)` borrows `s` itself, not
# its owned bytes) passed to a call assigned back to `s`: the temporary's
# loan ends when the call returns, before the store, and the argument is not
# an owned interior of `s`. The pinned Mojo prints `abc`.
def main():
    var s = String("abc")
    s = String(StringSpan(s))
    print(s)
