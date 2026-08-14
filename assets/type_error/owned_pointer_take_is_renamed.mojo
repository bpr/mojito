# expect: no method 'take'
from std.memory import OwnedPointer

def main():
    var p = OwnedPointer[Int](1)
    print(p^.take())
