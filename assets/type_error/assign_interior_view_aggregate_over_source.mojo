# An aggregate argument carries the origins of the views it holds: a
# `Holder` built over `s.rstrip()` borrows `s`'s owned bytes. The pinned
# Mojo rejects it with the same text.
# expect: aliasing values passed immutably to 'h' argument and constructed as a result in 'consume' call
struct Holder[o: ImmOrigin]:
    var v: StringSpan[Self.o]
    def __init__(out self, v: StringSpan[Self.o]):
        self.v = v

def consume(h: Holder) -> String:
    return String(h.v)

def main():
    var s = String("abc  ")
    s = consume(Holder(s.rstrip()))
    print(s)
