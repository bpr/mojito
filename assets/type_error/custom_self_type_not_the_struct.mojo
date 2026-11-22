# A custom receiver type must apply the enclosing struct.
# expect: 'self' argument must have type 'Box[T]', but actually has type 'Int'
@fieldwise_init
struct Box[T: AnyType]:
    var x: Int

    @__allow_legacy_custom_self_type
    def other(self: Int) -> Int:
        return self


def main():
    print(Box[Int](1).x)
