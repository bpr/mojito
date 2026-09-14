# "An owned interior of the destination" is a prefix test: a view of
# `b.s`'s bytes lies under `b`, so a call assigned back to the whole `b`
# aliases it. The pinned Mojo rejects it with the same text.
# expect: aliasing values passed immutably to 'v' argument and constructed as a result in 'rebox' call
struct Box:
    var s: String
    def __init__(out self, var s: String):
        self.s = s^

def rebox(v: StringSpan) -> Box:
    return Box(String(v))

def main():
    var b = Box("abc  ")
    b = rebox(b.s.rstrip())
    print(b.s)
