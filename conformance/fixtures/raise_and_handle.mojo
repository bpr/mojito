def fail() raises:
    raise Error("expected")

def main():
    try:
        fail()
    except:
        print("caught")
