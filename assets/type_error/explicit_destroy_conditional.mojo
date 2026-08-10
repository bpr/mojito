# expect: was conditionally destroyed
@explicit_destroy("close the resource")
struct Resource(Deinitable where False):
    def __init__(out self):
        pass

    def close(deinit self):
        pass

def main():
    var resource = Resource()
    if True:
        resource^.close()
