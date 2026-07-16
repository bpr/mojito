def add_original(mut destination: Int, original: Int):
    destination = destination + original

def main():
    var value = 5
    add_original(value, value)
    print(value)
