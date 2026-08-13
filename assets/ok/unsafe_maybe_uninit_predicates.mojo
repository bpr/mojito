# UnsafeMaybeUninit lifecycle facts: its destructor is trivial for every
# payload, move/copy triviality follows the payload's, and RegisterPassable
# is conditional on the payload. (Negative conforms_to shapes over struct
# payloads hit the pre-existing comptime Index-vs-TypeApply gap and are
# pinned by the type_error fixtures instead.)
from std.memory import UnsafeMaybeUninit

def main():
    comptime if IsTriviallyCopyable[UnsafeMaybeUninit[Int]]:
        print("int copy trivial")
    comptime if IsTriviallyMovable[UnsafeMaybeUninit[Int]]:
        print("int move trivial")
    comptime if IsTriviallyDeinitable[UnsafeMaybeUninit[Int]]:
        print("int deinit trivial")
    comptime if not IsTriviallyCopyable[UnsafeMaybeUninit[String]]:
        print("string copy nontrivial")
    comptime if not IsTriviallyMovable[UnsafeMaybeUninit[String]]:
        print("string move nontrivial")
    comptime if IsTriviallyDeinitable[UnsafeMaybeUninit[String]]:
        print("string deinit still trivial")
    comptime if conforms_to(UnsafeMaybeUninit[Int], RegisterPassable):
        print("int register passable")
