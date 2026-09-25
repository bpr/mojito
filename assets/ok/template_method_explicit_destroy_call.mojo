# A per-instantiation method clone inherits its checked template's facts when
# the body consumes a value through a method that takes its receiver
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `consuming calls`): a named `deinit self` destructor of a closed struct on a
# local, one of a struct built over the struct's parameter on a field of a
# consumed `self`, a `var self` method, and a `deinit self` trait requirement
# through a bound, whose instance selects either a named destructor or a
# plain consuming method. Which methods a struct declares as destructors does
# not change with its arguments; through a bound, the instance's struct is
# asked again.
trait Finishable:
    def finish(deinit self) -> Int:
        ...


@explicit_destroy("finish the ticket")
struct Ticket(Finishable, Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def finish(deinit self) -> Int:
        return self.id


struct Stub(Finishable, Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def finish(deinit self) -> Int:
        return self.id * 2


@explicit_destroy("release the lease")
struct Lease[T: Copyable & Deinitable](Movable, Deinitable where False):
    var item: Self.T
    var days: Int

    def __init__(out self, var item: Self.T, days: Int):
        self.item = item^
        self.days = days

    def release(deinit self) -> Int:
        return self.days

    def extend(var self, days: Int) -> Self:
        self.days += days
        return self^


struct Desk[T: Copyable & Deinitable](Movable):
    var count: Int

    def __init__(out self):
        self.count = 0

    def spend(mut self, n: Int) -> Int:
        self.count += 1
        var ticket = Ticket(n + self.count)
        return ticket^.finish()

    def lease(self, var item: Self.T, days: Int) -> Int:
        var lease = Lease[Self.T](item^, days)
        var longer = lease^.extend(1)
        return longer^.release()


@explicit_destroy("break the seal")
struct Seal[T: AnyType](Movable, Deinitable where False):
    var mark: Int

    def __init__(out self, mark: Int):
        self.mark = mark

    def open(deinit self) -> Int:
        return self.mark


@explicit_destroy("close the ledger")
struct Ledger[T: AnyType](Movable, Deinitable where False):
    var seal: Seal[Self.T]
    var entries: Int

    def __init__(out self, var seal: Seal[Self.T], entries: Int):
        self.seal = seal^
        self.entries = entries

    def close(deinit self) -> Int:
        return self.seal^.open() + self.entries


struct Clerk[T: Finishable & Movable](Movable):
    var handled: Int

    def __init__(out self):
        self.handled = 0

    def handle(mut self, var item: Self.T) -> Int:
        self.handled += 1
        return item^.finish()


def main():
    var a = Desk[Int]()
    var b = Desk[String]()
    print(a.spend(1), b.spend(2), a.spend(3))
    print(a.lease(4, 5), b.lease("x", 6))
    var ints = Ledger[Int](Seal[Int](7), 1)
    var strings = Ledger[String](Seal[String](8), 1)
    print(ints^.close(), strings^.close())
    var tickets = Clerk[Ticket]()
    var stubs = Clerk[Stub]()
    print(tickets.handle(Ticket(10)), stubs.handle(Stub(11)), tickets.handled)
