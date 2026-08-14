# expect: Optional.deinit_assert_empty on a non-empty Optional
from std.optional import Optional

def main():
    var value = Optional[Int](7)
    value^.deinit_assert_empty()
