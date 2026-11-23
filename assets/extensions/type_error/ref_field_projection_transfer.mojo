# expect: expression does not designate a value with an origin
# A field projected through a `ref` field lives in the referent, so `^` out
# of it reaches no tracked root of the holder.
@fieldwise_init
struct Named(Movable):
    var name: String


struct Holder[o: MutOrigin]:
    var r: ref[Self.o] Named

    def __init__(out self, ref[Self.o] n: Named):
        self.r = n


def take(var s: String):
    print(s)


def main():
    var n = Named("hi")
    var h = Holder(n)
    take(h.r.name^)
