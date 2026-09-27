# A value argument a nested generic `def` builds from its own parameter
# (`scaled[k + 1]()` inside `inner[k: Int]`) runs on the VM, and the native
# backend refuses it: "unsupported unresolved value parameter `k`". The nested
# `def` is called through its closure and binds no `k` as an instance
# constant. Filed from the forwarded value argument work (roadmap section 2);
# the pinned Mojo runs it.
def scaled[n: Int]() -> Int:
    return n


def outer[n: Int]() -> Int:
    def inner[k: Int]() -> Int:
        return scaled[k + 1]()

    return inner[10]()


def main():
    print(outer[1]())
