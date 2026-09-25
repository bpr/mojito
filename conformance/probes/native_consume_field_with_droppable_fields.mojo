# A named destructor called on a field of a consumed `self`
# (`self.lease^.release()` in a `deinit self` method) runs on the VM but the
# native backend refuses it when the field's type has droppable fields: "in
# `Ledger$mono$TString.close$y6:String`: unsupported place consumption with
# droppable fields". Pliron lowers `ConsumePlace` only for a type whose fields
# need no destruction. Filed from the explicit-destructor derivation work
# (roadmap section 2); the pinned Mojo runs it.
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
