# Variadic-generic struct with a Writable-bounded pack and a streaming
# write_to: the shape the self-hosted lazy TString relies on.  The pack
# constructor moves each element into Tuple storage; write_to fans the
# captured elements out to the writer with a comptime-unrolled loop.
struct Lazy[*Ts: Movable & Writable](Movable, Writable):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple(*args^)

    def write_to(self, mut writer: Some[Writer]):
        comptime for i in range(len(Ts)):
            # The unrolled iterations share one scope, so the ref binding
            # needs a nested block; the ref read keeps non-Copyable
            # elements legal where a value read would demand a copy.
            if True:
                ref element = self.storage[i]
                writer.write(element)


struct Once(Movable, Writable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def write_to(self, mut writer: Some[Writer]):
        writer.write("once#", self.tag)


def main():
    var lazy = Lazy[String, Int]("x=", 42)
    print(lazy)
    var s: String = String(lazy)
    print(s, len(s))
    var mixed = Lazy[String, Once, String]("[", Once(7), "]")
    print(mixed)
