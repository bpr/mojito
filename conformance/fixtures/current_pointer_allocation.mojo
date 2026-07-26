def main():
    var value = alloc[Int](1)
    value.unsafe_write(42)
    print(value[])
    value.free()
