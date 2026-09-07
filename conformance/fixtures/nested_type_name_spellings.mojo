# `_unqualified_type_name` spellings shared with current Mojo: a minted
# value specialization spells its baked key, a value argument spells
# `value : Type` while a `Bool` argument spells bare, at the top level and
# nested in another application.
from std.reflection.type_info import _unqualified_type_name
from std.hashlib import default_hasher

struct Box[T: Copyable, n: Int](Copyable, Movable):
    var v: Int

    def __init__(out self):
        self.v = 0

struct Flag[b: Bool](Copyable, Movable):
    var v: Int

    def __init__(out self):
        self.v = 0

def main():
    print(_unqualified_type_name[default_hasher]())
    print(_unqualified_type_name[Box[Int, 3]]())
    print(_unqualified_type_name[Flag[True]]())
    print(_unqualified_type_name[Optional[List[Int]]]())
