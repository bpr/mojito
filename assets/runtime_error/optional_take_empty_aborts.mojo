# expect: Optional.take on an empty Optional
from std.collections.optional import Optional

def main():
    var empty = Optional[Int]()
    print(empty.take())
