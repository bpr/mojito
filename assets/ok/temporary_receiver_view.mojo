# A view-returning method called on an owning temporary receiver: the
# temporary lives as long as the view that borrows it, so iterating or
# measuring the result in the same statement reads live storage. The checker
# materializes such a receiver into an anonymous owned binding, which is what
# gives the view's loan a real place.
def main():
    for c in String("abc").codepoints():
        print(c)
    for g in String("abc").__reversed__():
        print(g)
    print(len(String("abc").__reversed__()))
    for b in String("abc").as_bytes():
        print(b)
    print(String("a,b,c").split(",")[1])
    for g in String("xyz").graphemes():
        print(g)
