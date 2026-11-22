# A custom receiver type binds argument 0 exactly: no conversion reaches it.
# expect: value passed to 'self' cannot be converted from 'Box[String]' to 'Box[Int]'
@fieldwise_init
struct Box[T: AnyType]:
    var x: Int

    @__allow_legacy_custom_self_type
    def only_int(self: Box[Int]) -> Int:
        return self.x + 1


def main():
    var s = Box[String](4)
    print(s.only_int())
