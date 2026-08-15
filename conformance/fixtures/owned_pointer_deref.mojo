# Upstream's borrowed OwnedPointer dereference is the empty subscript
# (`p[]`); Mojito reserves the empty subscript for raw pointers — a
# recorded strict-subset gap (borrowed access spells `unsafe_ptr()[0]`).
from std.memory import OwnedPointer

def main():
    var p = OwnedPointer[Int](41)
    print(p[])
    print(p^.into_inner())
