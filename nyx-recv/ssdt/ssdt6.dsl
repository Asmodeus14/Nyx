/*
 * Intel ACPI Component Architecture
 * AML/ASL+ Disassembler version 20230628 (64-bit version)
 * Copyright (c) 2000 - 2023 Intel Corporation
 * 
 * Disassembling to symbolic ASL+ operators
 *
 * Disassembly of ssdt6.dat, Sat Sep  5 08:00:22 2026
 *
 * Original Table Header:
 *     Signature        "SSDT"
 *     Length           0x00000156 (342)
 *     Revision         0x02
 *     Checksum         0x33
 *     OEM ID           "Dell  "
 *     OEM Table ID     "ADebTabl"
 *     OEM Revision     0x00001000 (4096)
 *     Compiler ID      "INTL"
 *     Compiler Version 0x20160527 (538314023)
 */
DefinitionBlock ("", "SSDT", 2, "Dell  ", "ADebTabl", 0x00001000)
{
    External (OUTS, MethodObj)    // 1 Arguments

    Scope (\)
    {
        Name (DPTR, 0x66AAF000)
        Name (EPTR, 0x66ABF000)
        Name (CPTR, 0x66AAF020)
        Mutex (MMUT, 0x00)
        OperationRegion (ADBP, SystemIO, 0xB2, 0x02)
        Field (ADBP, ByteAcc, NoLock, Preserve)
        {
            B2PT,   8, 
            B3PT,   8
        }

        Method (MDBG, 1, Serialized)
        {
            OperationRegion (ADHD, SystemMemory, DPTR, 0x20)
            Field (ADHD, ByteAcc, NoLock, Preserve)
            {
                ASIG,   128, 
                ASIZ,   32, 
                ACHP,   32, 
                ACTP,   32, 
                SMIN,   8, 
                WRAP,   8, 
                SMMV,   8, 
                TRUN,   8
            }

            Local0 = Acquire (MMUT, 0x03E8)
            If ((Local0 == Zero))
            {
                OperationRegion (ABLK, SystemMemory, CPTR, 0x20)
                Field (ABLK, ByteAcc, NoLock, Preserve)
                {
                    AAAA,   256
                }

                ToHexString (Arg0, Local1)
                TRUN = Zero
                If ((SizeOf (Local1) >= 0x20))
                {
                    TRUN = One
                }

                Mid (Local1, Zero, 0x1F, AAAA) /* \MDBG.AAAA */
                CPTR += 0x20
                If ((CPTR >= EPTR))
                {
                    CPTR = (DPTR + 0x20)
                    WRAP = One
                }

                ACTP = CPTR /* \CPTR */
                If (SMMV)
                {
                    B2PT = SMIN /* \MDBG.SMIN */
                }
                Else
                {
                    OUTS (Arg0)
                }

                Release (MMUT)
            }

            Return (Local0)
        }
    }
}

