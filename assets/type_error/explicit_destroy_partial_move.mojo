# expect: was partially moved
@explicit_destroy("close the resource")
struct Resource:
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        pass

def consume(var value: Int):
    pass

def main():
    var resource = Resource(1)
    consume(resource.id^)
