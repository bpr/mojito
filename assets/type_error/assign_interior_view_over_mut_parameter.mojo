# The destination may be a `mut` parameter: a call assigned back through it
# whose view argument borrows the parameter's owned bytes aliases its result.
# The pinned Mojo rejects it with the same text.
# expect: aliasing values passed immutably to 'v' argument and constructed as a result in 'takes' call
def takes(v: StringSpan) -> String:
    return String(v)

def reset(mut s: String):
    s = takes(s.rstrip())

def main():
    var s = String("abc  ")
    reset(s)
    print(s)
