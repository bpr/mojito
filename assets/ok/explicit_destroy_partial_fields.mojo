# An `@explicit_destroy` aggregate whose field is itself linear: the field's
# named destructor runs from the aggregate's own, a reassignment destroys the
# field it replaces, and every value reaches an explicit destruction. Moving
# a field out (`rebuilt.child^.close()`) is legal only because the field is
# written back before the aggregate is used again.
@explicit_destroy("close the child")
struct Child(Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("child", self.id)

@explicit_destroy("finish the aggregate")
struct Aggregate(Deinitable where False):
    var child: Child
    var count: Int

    def __init__(out self, var child: Child, count: Int):
        self.child = child^
        self.count = count

    def finish(deinit self):
        print("count", self.count)
        self.child^.close()

def consume(value: Int):
    pass

def main():
    var aggregate = Aggregate(Child(7), 2)
    consume(aggregate.count)
    aggregate^.finish()

    var rebuilt = Aggregate(Child(8), 3)
    rebuilt.child^.close()
    rebuilt.child = Child(9)
    rebuilt.count = 4
    rebuilt^.finish()
