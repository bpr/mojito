# The rule is judged by origin, not by the view's lifetime: a named view of
# `s`'s owned bytes passed to a call assigned back to `s` is rejected even
# though the view is not used afterwards. The pinned Mojo rejects it with the
# same text.
# expect: aliasing values passed immutably to 'view' argument and constructed as a result in 'takes' call
def takes(view: StringSpan) -> String:
    return String(view)

def main():
    var s = String("abc  ")
    var r = s.rstrip()
    s = takes(r)
    print(s)
