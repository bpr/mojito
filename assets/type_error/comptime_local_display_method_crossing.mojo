# expect: cannot materialize comptime value of type 'Array[Int, Int(3)]'
# A method call over a local display binding is an application too, so its
# collection result is not implicitly copyable at run time.
def main():
    comptime l = [1, 2, 3]
    comptime c = l.copy()
    print(len(c))
