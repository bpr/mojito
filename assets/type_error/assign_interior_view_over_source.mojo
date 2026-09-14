# A call assigned straight back to the String whose owned bytes its view
# argument borrows (`s.rstrip()`). The pinned Mojo rejects it with the same
# text.
# expect: aliasing values passed immutably to 'v' argument and constructed as a result in 'takes' call
def takes(v: StringSpan) -> String:
    return String(v)

def main():
    var s = String("abc  ")
    s = takes(s.rstrip())
    print(s)
