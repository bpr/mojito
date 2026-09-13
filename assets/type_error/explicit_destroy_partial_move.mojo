# expect: is incomplete and cannot use a whole-value destructor
# The moved field owns storage: a transfer out of a trivial `Int` field has no
# effect in the pinned Mojo, so it would leave the value whole.
@explicit_destroy("close the resource")
struct Resource(Deinitable where False):
    var name: String

    def __init__(out self, var name: String):
        self.name = name^

    def close(deinit self):
        pass

def consume(var value: String):
    pass

def main():
    var resource = Resource(String("r"))
    consume(resource.name^)
    resource^.close()
