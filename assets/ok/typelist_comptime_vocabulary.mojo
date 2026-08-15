# The current-Mojo TypeList vocabulary in compile-time positions: the
# `of` constructor (optional Trait= keyword), `length` (Sized `len` too),
# per-element `any`/`all` predicates (builtin IsTrivially* or a Bool-bodied
# comptime alias), the `all_conforms_to` trait form, and `contains`.
comptime IsSmall[T: AnyType] = IsTriviallyCopyable[T]

def main():
    comptime tl = TypeList.of[Trait=AnyType, Int, Bool]()
    comptime if tl.length == 2:
        print("two")
    comptime if tl.all[IsSmall]():
        print("all small")
    comptime if tl.any[IsTriviallyCopyable]():
        print("some trivially copyable")
    comptime if tl.contains[Int]():
        print("has Int")
    comptime if not tl.contains[String]():
        print("no String")
    comptime if tl.all_conforms_to[Copyable]():
        print("all copyable")
    comptime if len(tl) == 2:
        print("len two")
