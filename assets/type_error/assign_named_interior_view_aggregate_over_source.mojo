# A named aggregate keeps the origins of the view it was built from, so
# passing it to a call assigned back to `s` aliases `s`'s owned bytes. The
# pinned Mojo rejects it with the same text.
# expect: aliasing values passed immutably to 'h' argument and constructed as a result in 'consume' call
struct Holder[o: ImmOrigin]:
    var v: StringSpan[Self.o]
    def __init__(out self, v: StringSpan[Self.o]):
        self.v = v

def consume(h: Holder) -> String:
    return String(h.v)

def main():
    var s = String("abc  ")
    var h = Holder(s.rstrip())
    s = consume(h)
    print(s)
