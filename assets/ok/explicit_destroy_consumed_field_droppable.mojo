# A named destructor called on a field of a consumed `self`
# (`self.lease^.release()` in a `deinit self` method) whose type has
# droppable fields: the field's own named destructor destroys the `String`
# it leaves behind, exactly once, on both backends.
@explicit_destroy("release the lease")
struct Lease[T: Copyable & Deinitable](Movable, Deinitable where False):
    var item: Self.T
    var days: Int

    def __init__(out self, var item: Self.T, days: Int):
        self.item = item^
        self.days = days

    def release(deinit self) -> Int:
        return self.days


@explicit_destroy("close the ledger")
struct Ledger[T: Copyable & Deinitable](Movable, Deinitable where False):
    var lease: Lease[Self.T]

    def __init__(out self, var lease: Lease[Self.T]):
        self.lease = lease^

    def close(deinit self) -> Int:
        return self.lease^.release()


def main():
    var ints = Ledger[Int](Lease[Int](7, 8))
    var strings = Ledger[String](Lease[String]("y", 9))
    print(ints^.close(), strings^.close())
