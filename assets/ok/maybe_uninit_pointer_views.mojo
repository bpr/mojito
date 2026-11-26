# `MaybeUninit[T]` has `T`'s layout, as upstream's `!pop.array<1, T>`:
# a pointer view between `T` and `MaybeUninit[T]` reads and writes the same
# bytes, and a byte-offset write into an uninitialized `MaybeUninit` (the
# `write_niche` shape of upstream's `UnsafeSingleNicheable`) initializes the
# payload bytes it covers.
from std.memory import MaybeUninit

@fieldwise_init
struct Loc(ImplicitlyCopyable):
    var file: Int
    var line: Int
    var col: Int

def write_niche(memory: MutPointer[MaybeUninit[Loc], _]):
    memory.unsafe_bitcast[Byte]().unsafe_offset(8).unsafe_bitcast[Int]()[] = -1

def isa_niche(memory: ImmPointer[MaybeUninit[Loc], _]) -> Bool:
    return memory.unsafe_bitcast[Byte]().unsafe_offset(8).unsafe_bitcast[Int]()[] == -1

def main():
    var x = 5
    print(Pointer(to=x).unsafe_bitcast[MaybeUninit[Int]]()[].unsafe_assume_init())
    var m = MaybeUninit[Int](7)
    print(Pointer(to=m).unsafe_bitcast[Int]()[])
    Pointer(to=m).unsafe_bitcast[Int]()[] = 9
    print(m.unsafe_assume_init())

    var empty = MaybeUninit[Loc]()
    write_niche(Pointer(to=empty))
    print(isa_niche(Pointer(to=empty)))
    var held = MaybeUninit[Loc](Loc(1, 2, 3))
    print(isa_niche(Pointer(to=held)))
    write_niche(Pointer(to=held))
    ref loc = held.unsafe_assume_init()
    print(isa_niche(Pointer(to=held)), loc.file, loc.line, loc.col)
