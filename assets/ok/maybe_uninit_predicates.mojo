# MaybeUninit lifecycle facts: copy and move triviality follow the payload's,
# and `RegisterPassable` is conditional on the payload. Deinit triviality is
# the payload's too, because `MaybeUninit` conforms to `Deinitable` only where
# the payload is trivially deinitable. (Negative conforms_to shapes over
# struct payloads hit the pre-existing comptime Index-vs-TypeApply gap and are
# pinned by the type_error fixtures instead.)
from std.memory import MaybeUninit
from std.traits import (
    IsTriviallyCopyable,
    IsTriviallyDeinitable,
    IsTriviallyMovable,
)

def main():
    comptime if IsTriviallyCopyable[MaybeUninit[Int]]:
        print("int copy trivial")
    comptime if IsTriviallyMovable[MaybeUninit[Int]]:
        print("int move trivial")
    comptime if IsTriviallyDeinitable[MaybeUninit[Int]]:
        print("int deinit trivial")
    comptime if not IsTriviallyCopyable[MaybeUninit[String]]:
        print("string copy nontrivial")
    comptime if not IsTriviallyDeinitable[MaybeUninit[String]]:
        print("string deinit nontrivial")
    comptime if conforms_to(MaybeUninit[Int], RegisterPassable):
        print("int register passable")
