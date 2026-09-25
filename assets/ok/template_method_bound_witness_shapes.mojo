# A per-instantiation method clone inherits its checked template's facts for
# a call through the struct parameter's bound whatever shape the instance's
# witness has (`docs/notes/instantiation-from-template.md`, obligation 16):
# a member of an overload set the arity selects, a `[H: Hasher]` binder a
# concrete hasher bakes into the per-call clone, a `mut self` requirement,
# and a `var self` one the receiver's `^` transfer consumes.
from std.hashlib import Hasher
from std.hashlib._ahash import AHasher


@fieldwise_init
struct Twice(Copyable, Deinitable, Hashable, Movable, Writable):
    var x: Int

    def __hash__(self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)

    def __hash__(self, mut hasher: Some[Hasher], salt: Int):
        self.x.__hash__(hasher)
        salt.__hash__(hasher)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("T", self.x)

    def write_to(self, mut writer: Some[Writer], width: Int):
        writer.write("T", self.x, "/", width)


@fieldwise_init
struct Baked(Copyable, Deinitable, Hashable, Movable, Writable):
    var x: Int

    def __hash__[H: Hasher](self, mut hasher: H):
        self.x.__hash__(hasher)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("B", self.x)


struct Holder[T: Copyable & Deinitable & Hashable & Writable](Hashable, Movable, Writable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def __hash__[H: Hasher](self, mut hasher: H):
        self.item.__hash__(hasher)

    def write_to(self, mut writer: Some[Writer]):
        self.item.write_to(writer)

    def feed(self, mut hasher: AHasher[SIMD[DType.uint64, 4](0)]):
        self.item.__hash__(hasher)

    def show(self) -> String:
        var text = String()
        self.item.write_to(text)
        return text


trait Tally:
    def bump(mut self, by: Int):
        ...

    def total(self) -> Int:
        ...

    def drain(var self) -> Int:
        ...


struct Counter(Copyable, Deinitable, Movable, Tally):
    var count: Int

    def __init__(out self, count: Int):
        self.count = count

    def bump(mut self, by: Int):
        self.count += by

    def total(self) -> Int:
        return self.count

    def drain(var self) -> Int:
        return self.count


struct Ledger[T: Copyable & Deinitable & Tally](Movable):
    var entry: Self.T

    def __init__(out self, var entry: Self.T):
        self.entry = entry^

    def step(mut self):
        self.entry.bump(2)

    def settle(self, var other: Self.T) -> Int:
        return other^.drain() + self.entry.total()


def digest[T: Copyable & Deinitable & Hashable & Writable](holder: Holder[T]) -> UInt64:
    var hasher = AHasher[SIMD[DType.uint64, 4](0)]()
    holder.feed(hasher)
    return hasher^.finish()


def main():
    var twice = Holder[Twice](Twice(3))
    var baked = Holder[Baked](Baked(3))
    var ints = Holder[Int](3)
    var strings = Holder[String](String("s"))
    print(twice, baked, ints, strings)
    print(twice.show(), baked.show(), ints.show(), strings.show())
    print(hash(twice) == hash(Holder[Twice](Twice(3))), hash(baked) == hash(ints))
    print(digest(twice) == digest(baked), digest(baked) == digest(ints))
    print(digest(strings) == digest(ints))
    var ledger = Ledger[Counter](Counter(1))
    ledger.step()
    ledger.step()
    print(ledger.settle(Counter(10)))
