# OwnedPointer's borrowed dereference is the empty subscript (`p[]`), its
# zero-argument `__getitem__`.
from std.memory import OwnedPointer

def main():
    var p = OwnedPointer[Int](41)
    print(p[])
    print(p^.into_inner())
