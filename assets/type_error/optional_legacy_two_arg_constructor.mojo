# expect: no constructor overload matches
from std.optional import Optional

def main():
    var o = Optional[Int](9, True)
    print(o.is_some())
