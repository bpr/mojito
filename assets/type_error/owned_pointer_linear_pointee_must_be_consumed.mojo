# expect: explicitly destroyed
from std.memory import OwnedPointer, Allocation, Layout, alloc

def main():
    var leaked = OwnedPointer[Allocation[Int]](alloc[Int](Layout[Int](count=1)))
