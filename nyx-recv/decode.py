import binascii

print("Reading dsdt.hex...")
with open("dsdt.hex", "r") as f:
    # Read the file and strip out any spaces or newlines
    hex_data = f.read().replace('\n', '').replace('\r', '').replace(' ', '')

print("Converting to binary...")
with open("dsdt.dat", "wb") as f:
    f.write(binascii.unhexlify(hex_data))

print("Success! dsdt.dat has been generated.")