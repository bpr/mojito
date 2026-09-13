# expect: was conditionally destroyed
# The condition is decided at run time: a constant `if True:` is folded by the
# pinned Mojo, which then sees an unconditional destroy and accepts.
@explicit_destroy("close the resource")
struct Resource(Deinitable where False):
    def __init__(out self):
        pass

    def close(deinit self):
        pass

def main():
    var flag = String("ab").byte_length() == 2
    var resource = Resource()
    if flag:
        resource^.close()
