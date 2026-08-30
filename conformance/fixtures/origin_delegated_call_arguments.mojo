# Upstream's argument-taking delegated-call origin expression
# (`ref[self.cursor.step(1).key]`). Both compilers print 7 (confirmed
# against the a79fbdf59f2 pin, 2026-08-30).
@fieldwise_init
struct Pair(Copyable, Movable):
    var key: Int
    var value: Int

@fieldwise_init
struct EntryCursor[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Pair, Self.o]

    def step(self, by: Int) -> ref[Self.o] Pair:
        return self.src[]

@fieldwise_init
struct KeyView[m: Bool, //, o: Origin[mut=m]]:
    var cursor: EntryCursor[Self.o]

    def key(self) -> ref[self.cursor.step(1).key] Int:
        return self.cursor.step(1).key

def main():
    var p = Pair(7, 70)
    var kv = KeyView(EntryCursor(Pointer(to=p)))
    print(kv.key())
