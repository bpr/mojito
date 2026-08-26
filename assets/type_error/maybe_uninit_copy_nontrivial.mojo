# MaybeUninit copies raw bits, so it is ImplicitlyCopyable only for a
# trivially copyable payload; String's copy is nontrivial.
# expect: cannot copy non-Copyable type
from std.memory import MaybeUninit

def main():
    var a = MaybeUninit[String](String("hi"))
    var b = a
    print(a.unsafe_assume_init())
    print(b.unsafe_assume_init())
