# MaybeUninit lifecycle facts: its destructor is trivial for every
# payload, move/copy triviality follows the payload's, and RegisterPassable
# is conditional on the payload. (Negative conforms_to shapes over struct
# payloads hit the pre-existing comptime Index-vs-TypeApply gap and are
# pinned by the type_error fixtures instead.)
from std.memory import MaybeUninit

def main():
    comptime if IsTriviallyCopyable[MaybeUninit[Int]]:
        print("int copy trivial")
    comptime if IsTriviallyMovable[MaybeUninit[Int]]:
        print("int move trivial")
    comptime if IsTriviallyDeinitable[MaybeUninit[Int]]:
        print("int deinit trivial")
    comptime if not IsTriviallyCopyable[MaybeUninit[String]]:
        print("string copy nontrivial")
    comptime if not IsTriviallyMovable[MaybeUninit[String]]:
        print("string move nontrivial")
    comptime if IsTriviallyDeinitable[MaybeUninit[String]]:
        print("string deinit still trivial")
    comptime if conforms_to(MaybeUninit[Int], RegisterPassable):
        print("int register passable")
