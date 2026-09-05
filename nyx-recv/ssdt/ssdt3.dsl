/*
 * Intel ACPI Component Architecture
 * AML/ASL+ Disassembler version 20230628 (64-bit version)
 * Copyright (c) 2000 - 2023 Intel Corporation
 * 
 * Disassembling to symbolic ASL+ operators
 *
 * Disassembly of ssdt3.dat, Sat Sep  5 08:00:22 2026
 *
 * Original Table Header:
 *     Signature        "SSDT"
 *     Length           0x0000426F (17007)
 *     Revision         0x01
 *     Checksum         0x47
 *     OEM ID           "DELL"
 *     OEM Table ID     "NvdTable"
 *     OEM Revision     0x00001000 (4096)
 *     Compiler ID      "INTL"
 *     Compiler Version 0x20160527 (538314023)
 */
DefinitionBlock ("", "SSDT", 1, "DELL", "NvdTable", 0x00001000)
{
    External (_SB_.CPPC, IntObj)
    External (_SB_.GGIV, MethodObj)    // 1 Arguments
    External (_SB_.GGOV, MethodObj)    // 1 Arguments
    External (_SB_.HWPV, UnknownObj)
    External (_SB_.PCI0, DeviceObj)
    External (_SB_.PCI0.LPCB.EC__.GPUP, FieldUnitObj)
    External (_SB_.PCI0.LPCB.ECDV.ECNV, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.PEG0, DeviceObj)
    External (_SB_.PCI0.PEG0.DEID, FieldUnitObj)
    External (_SB_.PCI0.PEG0.PEGP, DeviceObj)
    External (_SB_.PCI0.PEG0.PEGP._ADR, UnknownObj)
    External (_SB_.PCI0.PEG0.PEGP.CKNG, MethodObj)    // 0 Arguments
    External (_SB_.PCI0.PEG0.PEGP.HDAE, FieldUnitObj)
    External (_SB_.PCI0.PEG0.PEGP.HGPS, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.PEG0.PEGP.LTRE, FieldUnitObj)
    External (_SB_.PCI0.PEG0.PEGP.NGEN, IntObj)
    External (_SB_.PCI0.PEG0.PEGP.PWGD, FieldUnitObj)
    External (_SB_.PCI0.PEG0.PEGP.VGAR, FieldUnitObj)
    External (_SB_.PCI0.PEG0.PINI, MethodObj)    // 0 Arguments
    External (_SB_.PCI0.PEG0.PPBA, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.PEG1, DeviceObj)
    External (_SB_.PCI0.PEG1.PEGP, DeviceObj)
    External (_SB_.PCI0.PEG1.PINI, MethodObj)    // 0 Arguments
    External (_SB_.PCI0.PEG1.PPBA, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.PEG2, DeviceObj)
    External (_SB_.PCI0.PEG2.PEGP, DeviceObj)
    External (_SB_.PCI0.PEG2.PINI, MethodObj)    // 0 Arguments
    External (_SB_.PCI0.PEG2.PPBA, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.PGOF, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.PGON, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.RTDS, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.RTEN, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.SGPO, MethodObj)    // 5 Arguments
    External (_SB_.PR00._PSS, MethodObj)    // 0 Arguments
    External (_SB_.SGOV, MethodObj)    // 2 Arguments
    External (_SB_.SHPO, MethodObj)    // 2 Arguments
    External (AR02, UnknownObj)
    External (AR0A, UnknownObj)
    External (AR0B, UnknownObj)
    External (DLHR, UnknownObj)
    External (DLPW, UnknownObj)
    External (ECR1, UnknownObj)
    External (EEC1, UnknownObj)
    External (EEC2, UnknownObj)
    External (EECP, UnknownObj)
    External (GPRW, MethodObj)    // 2 Arguments
    External (HRA0, FieldUnitObj)
    External (HRA1, FieldUnitObj)
    External (HRA2, FieldUnitObj)
    External (HRE0, FieldUnitObj)
    External (HRE1, FieldUnitObj)
    External (HRE2, FieldUnitObj)
    External (HRG0, FieldUnitObj)
    External (HRG1, FieldUnitObj)
    External (HRG2, FieldUnitObj)
    External (LTRW, UnknownObj)
    External (LTRX, UnknownObj)
    External (LTRY, UnknownObj)
    External (LTRZ, UnknownObj)
    External (OBFA, UnknownObj)
    External (OBFX, UnknownObj)
    External (OBFY, UnknownObj)
    External (OBFZ, UnknownObj)
    External (ODV0, IntObj)
    External (OSYS, UnknownObj)
    External (P0UB, UnknownObj)
    External (P0WK, UnknownObj)
    External (P1GP, FieldUnitObj)
    External (P1UB, UnknownObj)
    External (P1WK, UnknownObj)
    External (P2GP, FieldUnitObj)
    External (P2UB, UnknownObj)
    External (P2WK, UnknownObj)
    External (PBGE, FieldUnitObj)
    External (PD02, UnknownObj)
    External (PD0A, UnknownObj)
    External (PD0B, UnknownObj)
    External (PICM, UnknownObj)
    External (PNOT, MethodObj)    // 0 Arguments
    External (PWA0, FieldUnitObj)
    External (PWA1, FieldUnitObj)
    External (PWA2, FieldUnitObj)
    External (PWE0, FieldUnitObj)
    External (PWE1, FieldUnitObj)
    External (PWE2, FieldUnitObj)
    External (PWG0, FieldUnitObj)
    External (PWG1, FieldUnitObj)
    External (PWG2, FieldUnitObj)
    External (SBN0, UnknownObj)
    External (SBN1, UnknownObj)
    External (SBN2, UnknownObj)
    External (SCLK, UnknownObj)
    External (SGGP, UnknownObj)
    External (SGMD, FieldUnitObj)
    External (SMSL, UnknownObj)
    External (SNSL, UnknownObj)
    External (SPCO, MethodObj)    // 2 Arguments
    External (XBAS, UnknownObj)

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        OperationRegion (VBOR, SystemMemory, 0x67E77018, 0x00040004)
        Field (VBOR, DWordAcc, Lock, Preserve)
        {
            RVBS,   32, 
            VBS1,   262144, 
            VBS2,   262144, 
            VBS3,   262144, 
            VBS4,   262144, 
            VBS5,   262144, 
            VBS6,   262144, 
            VBS7,   262144, 
            VBS8,   262144
        }
    }

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        OperationRegion (SGOP, SystemMemory, 0x67EE3F98, 0x00000027)
        Field (SGOP, AnyAcc, Lock, Preserve)
        {
            XBAS,   32, 
            EBAS,   32, 
            EECP,   32, 
            DBPA,   32, 
            SGGP,   8, 
            SGMD,   8, 
            APDT,   32, 
            AHDT,   32, 
            IHDT,   32, 
            DSSV,   32, 
            NVVD,   32, 
            OPTF,   8
        }
    }

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        OperationRegion (NOPR, SystemMemory, 0x67E74018, 0x00002023)
        Field (NOPR, AnyAcc, Lock, Preserve)
        {
            DHPS,   8, 
            DPCS,   8, 
            GPSS,   8, 
            VENS,   8, 
            NBCS,   8, 
            GC6S,   8, 
            NVSR,   8, 
            SLVS,   8, 
            PBCM,   8, 
            EXPM,   8, 
            MXBS,   32, 
            MXMB,   32768, 
            SMXS,   32, 
            SMXB,   32768, 
            FBEN,   32, 
            ENVT,   32, 
            DMMP,   32, 
            DLRP,   32, 
            DISM,   8
        }
    }

    Scope (\_SB.PCI0.PEG0)
    {
        Name (WKEN, Zero)
        OperationRegion (PEGR, PCI_Config, 0xC0, 0x30)
        Field (PEGR, DWordAcc, NoLock, Preserve)
        {
            Offset (0x02), 
            PSTS,   1, 
            Offset (0x2C), 
            GENG,   1, 
                ,   1, 
            PMEG,   1
        }

        Method (_PRW, 0, NotSerialized)  // _PRW: Power Resources for Wake
        {
            Return (GPRW (0x69, 0x04))
        }

        Method (HPME, 0, Serialized)
        {
            PSTS = One
        }

        Method (_PRT, 0, NotSerialized)  // _PRT: PCI Routing Table
        {
            If (PICM)
            {
                Return (AR02) /* External reference */
            }

            Return (PD02) /* External reference */
        }

        Method (_INI, 0, NotSerialized)  // _INI: Initialize
        {
            If (PRES ())
            {
                LTRS = LTRX /* External reference */
                OBFS = OBFX /* External reference */
                If (CondRefOf (PINI))
                {
                    PINI ()
                }
            }
        }

        Name (LTRV, Package (0x04)
        {
            Zero, 
            Zero, 
            Zero, 
            Zero
        })
        OperationRegion (PXCS, PCI_Config, Zero, 0x0480)
        Field (PXCS, AnyAcc, NoLock, Preserve)
        {
            VDID,   32
        }

        Method (PRES, 0, NotSerialized)
        {
            If ((VDID == Ones))
            {
                Return (Zero)
            }
            Else
            {
                Return (One)
            }
        }

        Name (LNRD, Zero)
        Method (UPRD, 1, Serialized)
        {
            If ((Arg0 <= 0x2710))
            {
                LNRD = Arg0
            }

            Return (LNRD) /* \_SB_.PCI0.PEG0.LNRD */
        }

        Method (_DSM, 4, Serialized)  // _DSM: Device-Specific Method
        {
            If ((Arg0 == ToUUID ("e5c937d0-3553-4d7a-9117-ea4d19c3434d") /* Device Labeling Interface */))
            {
                Switch (ToInteger (Arg2))
                {
                    Case (Zero)
                    {
                        Name (DSMF, Buffer (0x02)
                        {
                             0x00, 0x00                                       // ..
                        })
                        CreateBitField (DSMF, Zero, FUN0)
                        CreateBitField (DSMF, 0x04, FUN4)
                        CreateBitField (DSMF, 0x06, FUN6)
                        CreateBitField (DSMF, 0x08, FUN8)
                        CreateBitField (DSMF, 0x09, FUN9)
                        CreateBitField (DSMF, 0x0A, FUNA)
                        CreateBitField (DSMF, 0x0B, FUNB)
                        FUN0 = One
                        If ((Arg1 >= 0x02))
                        {
                            If (LTRS)
                            {
                                FUN6 = One
                            }

                            If (OBFS)
                            {
                                FUN4 = One
                            }
                        }

                        If ((Arg1 >= 0x03))
                        {
                            If (ECR1)
                            {
                                FUN8 = One
                            }

                            If (ECR1)
                            {
                                FUN9 = One
                            }
                        }

                        If ((Arg1 >= 0x04))
                        {
                            If (CondRefOf (PPBA))
                            {
                                FUNA = One
                            }

                            FUNB = One
                        }

                        Return (DSMF) /* \_SB_.PCI0.PEG0._DSM.DSMF */
                    }
                    Case (0x04)
                    {
                        If ((Arg1 >= 0x02))
                        {
                            If (OBFS)
                            {
                                Return (Buffer (0x10)
                                {
                                    /* 0000 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
                                    /* 0008 */  0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00   // ........
                                })
                            }
                            Else
                            {
                                Return (Buffer (0x10)
                                {
                                    /* 0000 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
                                    /* 0008 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00   // ........
                                })
                            }
                        }
                    }
                    Case (0x06)
                    {
                        If ((Arg1 >= 0x02))
                        {
                            If (LTRS)
                            {
                                LTRV [Zero] = ((SMSL >> 0x0A) & 0x07)
                                LTRV [One] = (SMSL & 0x03FF)
                                LTRV [0x02] = ((SNSL >> 0x0A) & 0x07)
                                LTRV [0x03] = (SNSL & 0x03FF)
                                Return (LTRV) /* \_SB_.PCI0.PEG0.LTRV */
                            }
                            Else
                            {
                                Return (Zero)
                            }
                        }
                    }
                    Case (0x08)
                    {
                        If ((ECR1 == One))
                        {
                            If ((Arg1 >= 0x03))
                            {
                                Return (One)
                            }
                        }
                    }
                    Case (0x09)
                    {
                        If ((ECR1 == One))
                        {
                            If ((Arg1 >= 0x03))
                            {
                                Return (Package (0x05)
                                {
                                    0xC350, 
                                    Ones, 
                                    Ones, 
                                    0xC350, 
                                    Ones
                                })
                            }
                        }
                    }
                    Case (0x0A)
                    {
                        If (CondRefOf (PPBA))
                        {
                            Return (PPBA (Arg3))
                        }
                    }
                    Case (0x0B)
                    {
                        Return (UPRD (Arg3))
                    }

                }
            }

            Return (Buffer (One)
            {
                 0x00                                             // .
            })
        }

        Method (_DSW, 3, NotSerialized)  // _DSW: Device Sleep Wake
        {
            If (Arg1)
            {
                WKEN = Zero
            }
            ElseIf ((Arg0 && Arg2))
            {
                WKEN = One
            }
            Else
            {
                WKEN = Zero
            }
        }

        Method (P0EW, 0, NotSerialized)
        {
            If (WKEN)
            {
                If ((SGGP != Zero))
                {
                    If ((SGGP == One))
                    {
                        \_SB.SGOV (P0WK, One)
                        \_SB.SHPO (P0WK, Zero)
                    }
                }
            }
        }

        Method (_S0W, 0, NotSerialized)  // _S0W: S0 Device Wake State
        {
            Return (0x04)
        }
    }

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        OperationRegion (PCIS, PCI_Config, Zero, 0x0100)
        Field (PCIS, AnyAcc, NoLock, Preserve)
        {
            PVID,   16, 
            PDID,   16
        }

        Method (_PRW, 0, NotSerialized)  // _PRW: Power Resources for Wake
        {
            Return (GPRW (0x69, 0x04))
        }
    }

    Scope (\_SB.PCI0.PEG1)
    {
        Name (WKEN, Zero)
        OperationRegion (PEGR, PCI_Config, 0xC0, 0x30)
        Field (PEGR, DWordAcc, NoLock, Preserve)
        {
            Offset (0x02), 
            PSTS,   1, 
            Offset (0x2C), 
            GENG,   1, 
                ,   1, 
            PMEG,   1
        }

        Method (_PRW, 0, NotSerialized)  // _PRW: Power Resources for Wake
        {
            Return (GPRW (0x69, 0x04))
        }

        Method (HPME, 0, Serialized)
        {
            PSTS = One
        }

        Method (_PRT, 0, NotSerialized)  // _PRT: PCI Routing Table
        {
            If (PICM)
            {
                Return (AR0A) /* External reference */
            }

            Return (PD0A) /* External reference */
        }

        Method (_INI, 0, NotSerialized)  // _INI: Initialize
        {
            If (PRES ())
            {
                LTRS = LTRY /* External reference */
                OBFS = OBFY /* External reference */
                If (CondRefOf (PINI))
                {
                    PINI ()
                }
            }
        }

        Name (LTRV, Package (0x04)
        {
            Zero, 
            Zero, 
            Zero, 
            Zero
        })
        OperationRegion (PXCS, PCI_Config, Zero, 0x0480)
        Field (PXCS, AnyAcc, NoLock, Preserve)
        {
            VDID,   32
        }

        Method (PRES, 0, NotSerialized)
        {
            If ((VDID == Ones))
            {
                Return (Zero)
            }
            Else
            {
                Return (One)
            }
        }

        Name (LNRD, Zero)
        Method (UPRD, 1, Serialized)
        {
            If ((Arg0 <= 0x2710))
            {
                LNRD = Arg0
            }

            Return (LNRD) /* \_SB_.PCI0.PEG1.LNRD */
        }

        Method (_DSM, 4, Serialized)  // _DSM: Device-Specific Method
        {
            If ((Arg0 == ToUUID ("e5c937d0-3553-4d7a-9117-ea4d19c3434d") /* Device Labeling Interface */))
            {
                Switch (ToInteger (Arg2))
                {
                    Case (Zero)
                    {
                        Name (DSMF, Buffer (0x02)
                        {
                             0x00, 0x00                                       // ..
                        })
                        CreateBitField (DSMF, Zero, FUN0)
                        CreateBitField (DSMF, 0x04, FUN4)
                        CreateBitField (DSMF, 0x06, FUN6)
                        CreateBitField (DSMF, 0x08, FUN8)
                        CreateBitField (DSMF, 0x09, FUN9)
                        CreateBitField (DSMF, 0x0A, FUNA)
                        CreateBitField (DSMF, 0x0B, FUNB)
                        FUN0 = One
                        If ((Arg1 >= 0x02))
                        {
                            If (LTRS)
                            {
                                FUN6 = One
                            }

                            If (OBFS)
                            {
                                FUN4 = One
                            }
                        }

                        If ((Arg1 >= 0x03))
                        {
                            If (ECR1)
                            {
                                FUN8 = One
                            }

                            If (ECR1)
                            {
                                FUN9 = One
                            }
                        }

                        If ((Arg1 >= 0x04))
                        {
                            If (CondRefOf (PPBA))
                            {
                                FUNA = One
                            }

                            FUNB = One
                        }

                        Return (DSMF) /* \_SB_.PCI0.PEG1._DSM.DSMF */
                    }
                    Case (0x04)
                    {
                        If ((Arg1 >= 0x02))
                        {
                            If (OBFS)
                            {
                                Return (Buffer (0x10)
                                {
                                    /* 0000 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
                                    /* 0008 */  0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00   // ........
                                })
                            }
                            Else
                            {
                                Return (Buffer (0x10)
                                {
                                    /* 0000 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
                                    /* 0008 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00   // ........
                                })
                            }
                        }
                    }
                    Case (0x06)
                    {
                        If ((Arg1 >= 0x02))
                        {
                            If (LTRS)
                            {
                                LTRV [Zero] = ((SMSL >> 0x0A) & 0x07)
                                LTRV [One] = (SMSL & 0x03FF)
                                LTRV [0x02] = ((SNSL >> 0x0A) & 0x07)
                                LTRV [0x03] = (SNSL & 0x03FF)
                                Return (LTRV) /* \_SB_.PCI0.PEG1.LTRV */
                            }
                            Else
                            {
                                Return (Zero)
                            }
                        }
                    }
                    Case (0x08)
                    {
                        If ((ECR1 == One))
                        {
                            If ((Arg1 >= 0x03))
                            {
                                Return (One)
                            }
                        }
                    }
                    Case (0x09)
                    {
                        If ((ECR1 == One))
                        {
                            If ((Arg1 >= 0x03))
                            {
                                Return (Package (0x05)
                                {
                                    0xC350, 
                                    Ones, 
                                    Ones, 
                                    0xC350, 
                                    Ones
                                })
                            }
                        }
                    }
                    Case (0x0A)
                    {
                        If (CondRefOf (PPBA))
                        {
                            Return (PPBA (Arg3))
                        }
                    }
                    Case (0x0B)
                    {
                        Return (UPRD (Arg3))
                    }

                }
            }

            Return (Buffer (One)
            {
                 0x00                                             // .
            })
        }

        Method (_DSW, 3, NotSerialized)  // _DSW: Device Sleep Wake
        {
            If (Arg1)
            {
                WKEN = Zero
            }
            ElseIf ((Arg0 && Arg2))
            {
                WKEN = One
            }
            Else
            {
                WKEN = Zero
            }
        }

        Method (P1EW, 0, NotSerialized)
        {
            If (WKEN)
            {
                If ((P1GP != Zero))
                {
                    If ((P1GP == One))
                    {
                        \_SB.SGOV (P1WK, One)
                        \_SB.SHPO (P1WK, Zero)
                    }
                }
            }
        }

        Method (_S0W, 0, NotSerialized)  // _S0W: S0 Device Wake State
        {
            Return (0x04)
        }
    }

    Scope (\_SB.PCI0.PEG1.PEGP)
    {
        OperationRegion (PCIS, PCI_Config, Zero, 0x0100)
        Field (PCIS, AnyAcc, NoLock, Preserve)
        {
            PVID,   16, 
            PDID,   16
        }

        Method (_PRW, 0, NotSerialized)  // _PRW: Power Resources for Wake
        {
            Return (GPRW (0x69, 0x04))
        }
    }

    Scope (\_SB.PCI0.PEG2)
    {
        Name (WKEN, Zero)
        OperationRegion (PEGR, PCI_Config, 0xC0, 0x30)
        Field (PEGR, DWordAcc, NoLock, Preserve)
        {
            Offset (0x02), 
            PSTS,   1, 
            Offset (0x2C), 
            GENG,   1, 
                ,   1, 
            PMEG,   1
        }

        Method (_PRW, 0, NotSerialized)  // _PRW: Power Resources for Wake
        {
            Return (GPRW (0x69, 0x04))
        }

        Method (HPME, 0, Serialized)
        {
            PSTS = One
        }

        Method (_PRT, 0, NotSerialized)  // _PRT: PCI Routing Table
        {
            If (PICM)
            {
                Return (AR0B) /* External reference */
            }

            Return (PD0B) /* External reference */
        }

        Method (_INI, 0, NotSerialized)  // _INI: Initialize
        {
            If (PRES ())
            {
                LTRS = LTRZ /* External reference */
                OBFS = OBFZ /* External reference */
                If (CondRefOf (PINI))
                {
                    PINI ()
                }
            }
        }

        Name (LTRV, Package (0x04)
        {
            Zero, 
            Zero, 
            Zero, 
            Zero
        })
        OperationRegion (PXCS, PCI_Config, Zero, 0x0480)
        Field (PXCS, AnyAcc, NoLock, Preserve)
        {
            VDID,   32
        }

        Method (PRES, 0, NotSerialized)
        {
            If ((VDID == Ones))
            {
                Return (Zero)
            }
            Else
            {
                Return (One)
            }
        }

        Name (LNRD, Zero)
        Method (UPRD, 1, Serialized)
        {
            If ((Arg0 <= 0x2710))
            {
                LNRD = Arg0
            }

            Return (LNRD) /* \_SB_.PCI0.PEG2.LNRD */
        }

        Method (_DSM, 4, Serialized)  // _DSM: Device-Specific Method
        {
            If ((Arg0 == ToUUID ("e5c937d0-3553-4d7a-9117-ea4d19c3434d") /* Device Labeling Interface */))
            {
                Switch (ToInteger (Arg2))
                {
                    Case (Zero)
                    {
                        Name (DSMF, Buffer (0x02)
                        {
                             0x00, 0x00                                       // ..
                        })
                        CreateBitField (DSMF, Zero, FUN0)
                        CreateBitField (DSMF, 0x04, FUN4)
                        CreateBitField (DSMF, 0x06, FUN6)
                        CreateBitField (DSMF, 0x08, FUN8)
                        CreateBitField (DSMF, 0x09, FUN9)
                        CreateBitField (DSMF, 0x0A, FUNA)
                        CreateBitField (DSMF, 0x0B, FUNB)
                        FUN0 = One
                        If ((Arg1 >= 0x02))
                        {
                            If (LTRS)
                            {
                                FUN6 = One
                            }

                            If (OBFS)
                            {
                                FUN4 = One
                            }
                        }

                        If ((Arg1 >= 0x03))
                        {
                            If (ECR1)
                            {
                                FUN8 = One
                            }

                            If (ECR1)
                            {
                                FUN9 = One
                            }
                        }

                        If ((Arg1 >= 0x04))
                        {
                            If (CondRefOf (PPBA))
                            {
                                FUNA = One
                            }

                            FUNB = One
                        }

                        Return (DSMF) /* \_SB_.PCI0.PEG2._DSM.DSMF */
                    }
                    Case (0x04)
                    {
                        If ((Arg1 >= 0x02))
                        {
                            If (OBFS)
                            {
                                Return (Buffer (0x10)
                                {
                                    /* 0000 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
                                    /* 0008 */  0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00   // ........
                                })
                            }
                            Else
                            {
                                Return (Buffer (0x10)
                                {
                                    /* 0000 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
                                    /* 0008 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00   // ........
                                })
                            }
                        }
                    }
                    Case (0x06)
                    {
                        If ((Arg1 >= 0x02))
                        {
                            If (LTRS)
                            {
                                LTRV [Zero] = ((SMSL >> 0x0A) & 0x07)
                                LTRV [One] = (SMSL & 0x03FF)
                                LTRV [0x02] = ((SNSL >> 0x0A) & 0x07)
                                LTRV [0x03] = (SNSL & 0x03FF)
                                Return (LTRV) /* \_SB_.PCI0.PEG2.LTRV */
                            }
                            Else
                            {
                                Return (Zero)
                            }
                        }
                    }
                    Case (0x08)
                    {
                        If ((ECR1 == One))
                        {
                            If ((Arg1 >= 0x03))
                            {
                                Return (One)
                            }
                        }
                    }
                    Case (0x09)
                    {
                        If ((ECR1 == One))
                        {
                            If ((Arg1 >= 0x03))
                            {
                                Return (Package (0x05)
                                {
                                    0xC350, 
                                    Ones, 
                                    Ones, 
                                    0xC350, 
                                    Ones
                                })
                            }
                        }
                    }
                    Case (0x0A)
                    {
                        If (CondRefOf (PPBA))
                        {
                            Return (PPBA (Arg3))
                        }
                    }
                    Case (0x0B)
                    {
                        Return (UPRD (Arg3))
                    }

                }
            }

            Return (Buffer (One)
            {
                 0x00                                             // .
            })
        }

        Method (_DSW, 3, NotSerialized)  // _DSW: Device Sleep Wake
        {
            If (Arg1)
            {
                WKEN = Zero
            }
            ElseIf ((Arg0 && Arg2))
            {
                WKEN = One
            }
            Else
            {
                WKEN = Zero
            }
        }

        Method (P2EW, 0, NotSerialized)
        {
            If (WKEN)
            {
                If ((P2GP != Zero))
                {
                    If ((P2GP == One))
                    {
                        \_SB.SGOV (P2WK, One)
                        \_SB.SHPO (P2WK, Zero)
                    }
                }
            }
        }

        Method (_S0W, 0, NotSerialized)  // _S0W: S0 Device Wake State
        {
            Return (0x04)
        }
    }

    Scope (\_SB.PCI0.PEG2.PEGP)
    {
        OperationRegion (PCIS, PCI_Config, Zero, 0x0100)
        Field (PCIS, AnyAcc, NoLock, Preserve)
        {
            PVID,   16, 
            PDID,   16
        }

        Method (_PRW, 0, NotSerialized)  // _PRW: Power Resources for Wake
        {
            Return (GPRW (0x69, 0x04))
        }
    }

    Scope (\_SB.PCI0)
    {
        Name (IVID, 0xFFFF)
        Name (PEBA, Zero)
        Name (PION, Zero)
        Name (PIOF, Zero)
        Name (PBUS, Zero)
        Name (PDEV, Zero)
        Name (PFUN, Zero)
        Name (EBUS, Zero)
        Name (EDEV, Zero)
        Name (EFN0, Zero)
        Name (EFN1, One)
        Name (LTRS, Zero)
        Name (OBFS, Zero)
        Name (INDX, Zero)
        Name (DSOF, 0x06)
        Name (CPOF, 0x34)
        Name (SBOF, 0x19)
        Name (ELC0, Zero)
        Name (ECP0, Ones)
        Name (H0VI, Zero)
        Name (H0DI, Zero)
        Name (ELC1, Zero)
        Name (ECP1, Ones)
        Name (H1VI, Zero)
        Name (H1DI, Zero)
        Name (ELC2, Zero)
        Name (ECP2, Ones)
        Name (H2VI, Zero)
        Name (H2DI, Zero)
        Name (TIDX, Zero)
        Name (OTSD, Zero)
        Name (MXPG, 0x03)
        Name (FBDL, Zero)
        Name (CBDL, Zero)
        Name (MBDL, Zero)
        Name (HSTR, Zero)
        Name (LREV, Zero)
        Name (TCNT, Zero)
        Name (LDLY, 0x012C)
        OperationRegion (OPG0, SystemMemory, \_SB.PCI0.PEG0.PEGP.DBPA, 0x1000)
        Field (OPG0, AnyAcc, NoLock, Preserve)
        {
            P0VI,   16, 
            P0DI,   16, 
            Offset (0x06), 
            DSO0,   16, 
            Offset (0x34), 
            CPO0,   8, 
            Offset (0xB0), 
                ,   4, 
            P0LD,   1, 
            Offset (0x11A), 
                ,   1, 
            P0VC,   1, 
            Offset (0x214), 
            Offset (0x216), 
            P0LS,   4, 
            Offset (0x248), 
                ,   7, 
            Q0L2,   1, 
            Q0L0,   1, 
            Offset (0x504), 
            HST0,   32, 
            P0TR,   1, 
            Offset (0x91C), 
                ,   31, 
            BSP1,   1, 
            Offset (0x93C), 
                ,   31, 
            BSP2,   1, 
            Offset (0x95C), 
                ,   31, 
            BSP3,   1, 
            Offset (0x97C), 
                ,   31, 
            BSP4,   1, 
            Offset (0x99C), 
                ,   31, 
            BSP5,   1, 
            Offset (0x9BC), 
                ,   31, 
            BSP6,   1, 
            Offset (0x9DC), 
                ,   31, 
            BSP7,   1, 
            Offset (0x9FC), 
                ,   31, 
            BSP8,   1, 
            Offset (0xC20), 
                ,   4, 
            P0AP,   2, 
            Offset (0xC38), 
                ,   3, 
            P0RM,   1, 
            Offset (0xC74), 
            P0LT,   4, 
            Offset (0xD0C), 
            LRV0,   32
        }

        OperationRegion (PCS0, SystemMemory, \_SB.PCI0.PEG0.PEGP.EBAS, 0xF0)
        Field (PCS0, DWordAcc, Lock, Preserve)
        {
            D0VI,   16, 
            Offset (0x2C), 
            S0VI,   16, 
            S0DI,   16
        }

        OperationRegion (CAP0, SystemMemory, (\_SB.PCI0.PEG0.PEGP.EBAS + EECP), 0x14)
        Field (CAP0, DWordAcc, NoLock, Preserve)
        {
            Offset (0x0C), 
            LCP0,   32, 
            LCT0,   16
        }

        OperationRegion (OPG1, SystemMemory, (XBAS + 0x9000), 0x1000)
        Field (OPG1, AnyAcc, NoLock, Preserve)
        {
            P1VI,   16, 
            P1DI,   16, 
            Offset (0x06), 
            DSO1,   16, 
            Offset (0x34), 
            CPO1,   8, 
            Offset (0xB0), 
                ,   4, 
            P1LD,   1, 
            Offset (0x11A), 
                ,   1, 
            P1VC,   1, 
            Offset (0x214), 
            Offset (0x216), 
            P1LS,   4, 
            Offset (0x248), 
                ,   7, 
            Q1L2,   1, 
            Q1L0,   1, 
            Offset (0x504), 
            HST1,   32, 
            P1TR,   1, 
            Offset (0xC20), 
                ,   4, 
            P1AP,   2, 
            Offset (0xC38), 
                ,   3, 
            P1RM,   1, 
            Offset (0xC74), 
            P1LT,   4, 
            Offset (0xD0C), 
            LRV1,   32
        }

        OperationRegion (PCS1, SystemMemory, (XBAS + (SBN1 << 0x14)), 0xF0)
        Field (PCS1, DWordAcc, Lock, Preserve)
        {
            D1VI,   16, 
            Offset (0x2C), 
            S1VI,   16, 
            S1DI,   16
        }

        OperationRegion (CAP1, SystemMemory, ((XBAS + (SBN1 << 0x14)) + EEC1), 0x14)
        Field (CAP1, DWordAcc, NoLock, Preserve)
        {
            Offset (0x0C), 
            LCP1,   32, 
            LCT1,   16
        }

        OperationRegion (OPG2, SystemMemory, (XBAS + 0xA000), 0x1000)
        Field (OPG2, AnyAcc, NoLock, Preserve)
        {
            P2VI,   16, 
            P2DI,   16, 
            Offset (0x06), 
            DSO2,   16, 
            Offset (0x34), 
            CPO2,   8, 
            Offset (0xB0), 
                ,   4, 
            P2LD,   1, 
            Offset (0x11A), 
                ,   1, 
            P2VC,   1, 
            Offset (0x214), 
            Offset (0x216), 
            P2LS,   4, 
            Offset (0x248), 
                ,   7, 
            Q2L2,   1, 
            Q2L0,   1, 
            Offset (0x504), 
            HST2,   32, 
            P2TR,   1, 
            Offset (0xC20), 
                ,   4, 
            P2AP,   2, 
            Offset (0xC38), 
                ,   3, 
            P2RM,   1, 
            Offset (0xC74), 
            P2LT,   4, 
            Offset (0xD0C), 
            LRV2,   32
        }

        OperationRegion (PCS2, SystemMemory, (XBAS + (SBN2 << 0x14)), 0xF0)
        Field (PCS2, DWordAcc, Lock, Preserve)
        {
            D2VI,   16, 
            Offset (0x2C), 
            S2VI,   16, 
            S2DI,   16
        }

        OperationRegion (CAP2, SystemMemory, ((XBAS + (SBN2 << 0x14)) + EEC2), 0x14)
        Field (CAP2, DWordAcc, NoLock, Preserve)
        {
            Offset (0x0C), 
            LCP2,   32, 
            LCT2,   16
        }

        Method (MMRD, 5, Serialized)
        {
            Local7 = Arg0
            Local7 |= (Arg1 << 0x14)
            Local7 |= (Arg2 << 0x0F)
            Local7 |= (Arg3 << 0x0C)
            Local7 |= Arg4
            OperationRegion (PCI0, SystemMemory, Local7, 0x04)
            Field (PCI0, ByteAcc, NoLock, Preserve)
            {
                TEMP,   32
            }

            Return (TEMP) /* \_SB_.PCI0.MMRD.TEMP */
        }

        Method (GULC, 1, NotSerialized)
        {
            Local7 = MMRD (PEBA, PBUS, PDEV, PFUN, 0xAC)
            Local7 >>= 0x04
            Local7 &= 0x3F
            Local6 = Arg0
            Local6 >>= 0x04
            Local6 &= 0x3F
            If ((Local7 > Local6))
            {
                Local0 = (Local7 - Local6)
            }
            Else
            {
                Local0 = Zero
            }

            Return (Local0)
        }

        Method (GMXB, 1, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                HSTR = HST0 /* \_SB_.PCI0.HST0 */
            }
            ElseIf ((Arg0 == One))
            {
                HSTR = HST1 /* \_SB_.PCI0.HST1 */
            }
            ElseIf ((Arg0 == 0x02))
            {
                HSTR = HST2 /* \_SB_.PCI0.HST2 */
            }

            HSTR >>= 0x10
            HSTR &= 0x03
            If ((Arg0 == Zero))
            {
                If ((HSTR == 0x03))
                {
                    Local0 = 0x08
                }
                Else
                {
                    Local0 = 0x04
                }
            }
            ElseIf ((Arg0 == One))
            {
                If ((HSTR == 0x02))
                {
                    Local0 = 0x04
                }
                ElseIf ((HSTR == Zero))
                {
                    Local0 = 0x02
                }
            }
            ElseIf ((Arg0 == 0x02))
            {
                If ((HSTR == Zero))
                {
                    Local0 = 0x02
                }
                ElseIf ((HSTR == One))
                {
                    Local0 = 0x02
                }
            }

            Return (Local0)
        }

        Method (PUAB, 1, NotSerialized)
        {
            FBDL = Zero
            CBDL = Zero
            If ((Arg0 == Zero))
            {
                HSTR = HST0 /* \_SB_.PCI0.HST0 */
                LREV = LRV0 /* \_SB_.PCI0.LRV0 */
            }
            ElseIf ((Arg0 == One))
            {
                HSTR = HST1 /* \_SB_.PCI0.HST1 */
                LREV = LRV1 /* \_SB_.PCI0.LRV1 */
            }
            ElseIf ((Arg0 == 0x02))
            {
                HSTR = HST2 /* \_SB_.PCI0.HST2 */
                LREV = LRV2 /* \_SB_.PCI0.LRV2 */
            }

            HSTR >>= 0x10
            HSTR &= 0x03
            LREV >>= 0x14
            LREV &= One
            If ((Arg0 == Zero))
            {
                If ((HSTR == 0x03))
                {
                    FBDL = Zero
                    CBDL = 0x08
                }
                ElseIf ((LREV == Zero))
                {
                    FBDL = Zero
                    CBDL = 0x04
                }
                Else
                {
                    FBDL = 0x04
                    CBDL = 0x04
                }
            }
            ElseIf ((Arg0 == One))
            {
                If ((HSTR == 0x02))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = 0x04
                        CBDL = 0x04
                    }
                    Else
                    {
                        FBDL = Zero
                        CBDL = 0x04
                    }
                }
                ElseIf ((HSTR == Zero))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = 0x04
                        CBDL = 0x02
                    }
                    Else
                    {
                        FBDL = 0x02
                        CBDL = 0x02
                    }
                }
            }
            ElseIf ((Arg0 == 0x02))
            {
                If ((HSTR == Zero))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = 0x06
                        CBDL = 0x02
                    }
                    Else
                    {
                        FBDL = Zero
                        CBDL = 0x02
                    }
                }
                ElseIf ((HSTR == One))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = 0x06
                        CBDL = 0x02
                    }
                    Else
                    {
                        FBDL = Zero
                        CBDL = 0x02
                    }
                }
            }

            INDX = One
            If ((CBDL != Zero))
            {
                While ((INDX <= CBDL))
                {
                    If ((P0VI == IVID)){}
                    ElseIf ((P0VI != IVID))
                    {
                        If ((FBDL == Zero))
                        {
                            BSP1 = Zero
                        }

                        If ((FBDL == One))
                        {
                            BSP2 = Zero
                        }

                        If ((FBDL == 0x02))
                        {
                            BSP3 = Zero
                        }

                        If ((FBDL == 0x03))
                        {
                            BSP4 = Zero
                        }

                        If ((FBDL == 0x04))
                        {
                            BSP5 = Zero
                        }

                        If ((FBDL == 0x05))
                        {
                            BSP6 = Zero
                        }

                        If ((FBDL == 0x06))
                        {
                            BSP7 = Zero
                        }

                        If ((FBDL == 0x07))
                        {
                            BSP8 = Zero
                        }
                    }

                    FBDL++
                    INDX++
                }
            }
        }

        Method (PDUB, 2, NotSerialized)
        {
            FBDL = Zero
            CBDL = Arg1
            If ((CBDL == Zero))
            {
                Return (Zero)
            }

            If ((Arg0 == Zero))
            {
                HSTR = HST0 /* \_SB_.PCI0.HST0 */
                LREV = LRV0 /* \_SB_.PCI0.LRV0 */
            }
            ElseIf ((Arg0 == One))
            {
                HSTR = HST1 /* \_SB_.PCI0.HST1 */
                LREV = LRV1 /* \_SB_.PCI0.LRV1 */
            }
            ElseIf ((Arg0 == 0x02))
            {
                HSTR = HST2 /* \_SB_.PCI0.HST2 */
                LREV = LRV2 /* \_SB_.PCI0.LRV2 */
            }

            HSTR >>= 0x10
            HSTR &= 0x03
            LREV >>= 0x14
            LREV &= One
            If ((Arg0 == Zero))
            {
                If ((HSTR == 0x03))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = (0x08 - CBDL)
                    }
                    Else
                    {
                        FBDL = Zero
                    }
                }
                ElseIf ((LREV == Zero))
                {
                    FBDL = (0x04 - CBDL)
                }
                Else
                {
                    FBDL = 0x04
                }
            }
            ElseIf ((Arg0 == One))
            {
                If ((HSTR == 0x02))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = (0x08 - CBDL)
                    }
                    Else
                    {
                        FBDL = Zero
                    }
                }
                ElseIf ((HSTR == Zero))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = (0x06 - CBDL)
                    }
                    Else
                    {
                        FBDL = 0x02
                    }
                }
            }
            ElseIf ((Arg0 == 0x02))
            {
                If ((HSTR == Zero))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = (0x08 - CBDL)
                    }
                    Else
                    {
                        FBDL = Zero
                    }
                }
                ElseIf ((HSTR == One))
                {
                    If ((LREV == Zero))
                    {
                        FBDL = (0x08 - CBDL)
                    }
                    Else
                    {
                        FBDL = Zero
                    }
                }
            }

            INDX = One
            While ((INDX <= CBDL))
            {
                If ((P0VI == IVID)){}
                ElseIf ((P0VI != IVID))
                {
                    If ((FBDL == Zero))
                    {
                        BSP1 = One
                    }

                    If ((FBDL == One))
                    {
                        BSP2 = One
                    }

                    If ((FBDL == 0x02))
                    {
                        BSP3 = One
                    }

                    If ((FBDL == 0x03))
                    {
                        BSP4 = One
                    }

                    If ((FBDL == 0x04))
                    {
                        BSP5 = One
                    }

                    If ((FBDL == 0x05))
                    {
                        BSP6 = One
                    }

                    If ((FBDL == 0x06))
                    {
                        BSP7 = One
                    }

                    If ((FBDL == 0x07))
                    {
                        BSP8 = One
                    }
                }

                FBDL++
                INDX++
            }
        }

        Method (SBDL, 1, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                If ((P0UB == Zero))
                {
                    Return (Zero)
                }
            }
            ElseIf ((Arg0 == One))
            {
                If ((P1UB == Zero))
                {
                    Return (Zero)
                }
            }
            ElseIf ((Arg0 == 0x02))
            {
                If ((P2UB == Zero))
                {
                    Return (Zero)
                }
            }
            Else
            {
                Return (Zero)
            }

            Return (One)
        }

        Method (GUBC, 1, NotSerialized)
        {
            Local7 = Zero
            If ((Arg0 == Zero))
            {
                Local6 = LCP0 /* \_SB_.PCI0.LCP0 */
            }
            ElseIf ((Arg0 == One))
            {
                Local6 = LCP1 /* \_SB_.PCI0.LCP1 */
            }
            ElseIf ((Arg0 == 0x02))
            {
                Local6 = LCP2 /* \_SB_.PCI0.LCP2 */
            }

            If ((Arg0 == Zero))
            {
                If ((P0UB == 0xFF))
                {
                    Local5 = GULC (Local6)
                    Local7 = (Local5 / 0x02)
                }
                ElseIf ((P0UB != Zero))
                {
                    Local7 = P0UB /* External reference */
                }
            }
            ElseIf ((Arg0 == One))
            {
                If ((P1UB == 0xFF))
                {
                    Local5 = GULC (Local6)
                    Local7 = (Local5 / 0x02)
                }
                ElseIf ((P1UB != Zero))
                {
                    Local7 = P1UB /* External reference */
                }
            }
            ElseIf ((Arg0 == 0x02))
            {
                If ((P2UB == 0xFF))
                {
                    Local5 = GULC (Local6)
                    Local7 = (Local5 / 0x02)
                }
                ElseIf ((P2UB != Zero))
                {
                    Local7 = P2UB /* External reference */
                }
            }

            Return (Local7)
        }

        Method (DIWK, 1, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                \_SB.PCI0.PEG0.P0EW ()
            }
            ElseIf ((Arg0 == One))
            {
                \_SB.PCI0.PEG1.P1EW ()
            }
            ElseIf ((Arg0 == 0x02))
            {
                \_SB.PCI0.PEG2.P2EW ()
            }
        }

        Method (GDEV, 1, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                Local1 = \_SB.PCI0.PEG0.PEGP.DBPA
                Local0 = ((Local1 & 0x000F8000) >> 0x0F)
            }
            ElseIf ((Arg0 == One))
            {
                Local0 = One
            }
            ElseIf ((Arg0 == 0x02))
            {
                Local0 = One
            }

            Return (Local0)
        }

        Method (GFUN, 1, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                Local0 = Zero
            }
            ElseIf ((Arg0 == One))
            {
                Local0 = One
            }
            ElseIf ((Arg0 == 0x02))
            {
                Local0 = 0x02
            }

            Return (Local0)
        }

        Method (CCHK, 2, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                Local7 = P0VI /* \_SB_.PCI0.P0VI */
            }
            ElseIf ((Arg0 == One))
            {
                Local7 = P1VI /* \_SB_.PCI0.P1VI */
            }
            ElseIf ((Arg0 == 0x02))
            {
                Local7 = P2VI /* \_SB_.PCI0.P2VI */
            }

            If ((Local7 == IVID))
            {
                Return (Zero)
            }

            If ((Arg0 != Zero))
            {
                Local7 = P0VI /* \_SB_.PCI0.P0VI */
                If ((Local7 == IVID))
                {
                    Return (Zero)
                }
            }

            If ((Arg1 == Zero))
            {
                If ((Arg0 == Zero))
                {
                    If ((SGPI (SGGP, PWE0, PWG0, PWA0) == Zero))
                    {
                        Return (Zero)
                    }
                }

                If ((Arg0 == One))
                {
                    If ((SGPI (P1GP, PWE1, PWG1, PWA1) == Zero))
                    {
                        Return (Zero)
                    }
                }

                If ((Arg0 == 0x02))
                {
                    If ((SGPI (P2GP, PWE2, PWG2, PWA2) == Zero))
                    {
                        Return (Zero)
                    }
                }
            }
            ElseIf ((Arg1 == One))
            {
                If ((Arg0 == Zero))
                {
                    If ((SGPI (SGGP, PWE0, PWG0, PWA0) == One))
                    {
                        Return (Zero)
                    }
                }

                If ((Arg0 == One))
                {
                    If ((SGPI (P1GP, PWE1, PWG1, PWA1) == One))
                    {
                        Return (Zero)
                    }
                }

                If ((Arg0 == 0x02))
                {
                    If ((SGPI (P2GP, PWE2, PWG2, PWA2) == One))
                    {
                        Return (Zero)
                    }
                }
            }

            Return (One)
        }

        Method (NTFY, 2, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                Notify (\_SB.PCI0.PEG0, Arg1)
            }
            ElseIf ((Arg0 == One))
            {
                Notify (\_SB.PCI0.PEG1, Arg1)
            }
            ElseIf ((Arg0 == 0x02))
            {
                Notify (\_SB.PCI0.PEG2, Arg1)
            }
        }

        Method (GDLY, 1, NotSerialized)
        {
            Local1 = Zero
            Local0 = (Arg0 * 0x0A)
            While ((Local1 < Local0))
            {
                Stall (0x64)
                Local1 += One
            }
        }

        Method (GPPR, 2, NotSerialized)
        {
            If ((Arg1 == Zero))
            {
                If ((Arg0 == Zero))
                {
                    SGPO (SGGP, HRE0, HRG0, HRA0, One)
                    GDLY (DLHR)
                    SGPO (SGGP, PWE0, PWG0, PWA0, Zero)
                }

                If ((Arg0 == One))
                {
                    SGPO (P1GP, HRE1, HRG1, HRA1, One)
                    GDLY (DLHR)
                    SGPO (P1GP, PWE1, PWG1, PWA1, Zero)
                }

                If ((Arg0 == 0x02))
                {
                    SGPO (P2GP, HRE2, HRG2, HRA2, One)
                    GDLY (DLHR)
                    SGPO (P2GP, PWE2, PWG2, PWA2, Zero)
                }
            }
            ElseIf ((Arg1 == One))
            {
                If ((Arg0 == Zero))
                {
                    SGPO (SGGP, PWE0, PWG0, PWA0, One)
                    Name (PGDC, Zero)
                    While ((PGDC != 0x03))
                    {
                        If ((\_SB.GGIV (0x03030012) == One))
                        {
                            PGDC += One
                        }

                        GDLY (One)
                    }

                    SGPO (SGGP, HRE0, HRG0, HRA0, Zero)
                    Sleep (DLHR)
                }

                If ((Arg0 == One))
                {
                    SGPO (P1GP, PWE1, PWG1, PWA1, One)
                    GDLY (DLPW)
                    SGPO (P1GP, HRE1, HRG1, HRA1, Zero)
                    Sleep (DLHR)
                }

                If ((Arg0 == 0x02))
                {
                    SGPO (P2GP, PWE2, PWG2, PWA2, One)
                    GDLY (DLPW)
                    SGPO (P2GP, HRE2, HRG2, HRA2, Zero)
                    Sleep (DLHR)
                }
            }
        }

        Method (SGPO, 5, Serialized)
        {
            If ((Arg3 == Zero))
            {
                Arg4 = ~Arg4
                Arg4 &= One
            }

            If ((Arg0 == One))
            {
                If (CondRefOf (\_SB.SGOV))
                {
                    \_SB.SGOV (Arg2, Arg4)
                }
            }
        }

        Method (SGPI, 4, Serialized)
        {
            If ((Arg0 == One))
            {
                If (CondRefOf (\_SB.GGOV))
                {
                    Local0 = \_SB.GGOV (Arg2)
                }
            }

            If ((Arg3 == Zero))
            {
                Local0 = ~Local0
                Local0 &= One
            }

            Return (Local0)
        }

        Method (RTEN, 1, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                Q0L0 = One
                Sleep (0x10)
                Local0 = Zero
                While (Q0L0)
                {
                    If ((Local0 > 0x04))
                    {
                        Break
                    }

                    Sleep (0x10)
                    Local0++
                }

                P0RM = Zero
                P0AP = Zero
            }
            ElseIf ((Arg0 == One))
            {
                Q1L0 = One
                Sleep (0x10)
                Local0 = Zero
                While (Q1L0)
                {
                    If ((Local0 > 0x04))
                    {
                        Break
                    }

                    Sleep (0x10)
                    Local0++
                }

                P1RM = Zero
                P1AP = Zero
            }
            ElseIf ((Arg0 == 0x02))
            {
                Q2L0 = One
                Sleep (0x10)
                Local0 = Zero
                While (Q2L0)
                {
                    If ((Local0 > 0x04))
                    {
                        Break
                    }

                    Sleep (0x10)
                    Local0++
                }

                P2RM = Zero
                P2AP = Zero
            }
        }

        Method (RTDS, 1, NotSerialized)
        {
            If ((Arg0 == Zero))
            {
                Q0L2 = One
                Sleep (0x10)
                Local0 = Zero
                While (Q0L2)
                {
                    If ((Local0 > 0x04))
                    {
                        Break
                    }

                    Sleep (0x10)
                    Local0++
                }

                P0RM = One
                P0AP = 0x03
            }
            ElseIf ((Arg0 == One))
            {
                Q1L2 = One
                Sleep (0x10)
                Local0 = Zero
                While (Q1L2)
                {
                    If ((Local0 > 0x04))
                    {
                        Break
                    }

                    Sleep (0x10)
                    Local0++
                }

                P1RM = One
                P1AP = 0x03
            }
            ElseIf ((Arg0 == 0x02))
            {
                Q2L2 = One
                Sleep (0x10)
                Local0 = Zero
                While (Q2L2)
                {
                    If ((Local0 > 0x04))
                    {
                        Break
                    }

                    Sleep (0x10)
                    Local0++
                }

                P2RM = One
                P2AP = 0x03
            }
        }
    }

    Scope (\_SB.PCI0.PEG0)
    {
        OperationRegion (RPCX, SystemMemory, \_SB.PCI0.PEG0.PEGP.DBPA, 0x1000)
        Field (RPCX, DWordAcc, NoLock, Preserve)
        {
            Offset (0x04), 
            CMDR,   8, 
            Offset (0x19), 
            PRBN,   8, 
            Offset (0x84), 
            D0ST,   2, 
            Offset (0xAA), 
            CEDR,   1, 
            Offset (0xB0), 
            ASPM,   2, 
                ,   2, 
            LNKD,   1, 
            Offset (0xC9), 
                ,   2, 
            LREN,   1, 
            Offset (0x216), 
            LNKS,   4
        }

        PowerResource (PG00, 0x00, 0x0000)
        {
            Name (_STA, One)  // _STA: Status
            Method (_ON, 0, Serialized)  // _ON_: Power On
            {
                If ((\_SB.PCI0.PEG0.PEGP.TDGC == One))
                {
                    If ((\_SB.PCI0.PEG0.PEGP.DGCX == 0x03))
                    {
                        \_SB.PCI0.PEG0.PEGP.GC6O ()
                    }
                    ElseIf ((\_SB.PCI0.PEG0.PEGP.DGCX == 0x04))
                    {
                        \_SB.PCI0.PEG0.PEGP.GC6O ()
                    }

                    \_SB.PCI0.PEG0.PEGP.TDGC = Zero
                    \_SB.PCI0.PEG0.PEGP.DGCX = Zero
                    _STA = One
                }
                Else
                {
                    PGON (Zero)
                    \_SB.PCI0.PEG0.CMDR = 0x07
                    \_SB.PCI0.PEG0.D0ST = Zero
                    \_SB.PCI0.PEG0.PEGP.SSSV = \_SB.PCI0.PEG0.PEGP.DSSV
                    If ((\_SB.PCI0.PEG0.PEGP.NGEN <= 0x11))
                    {
                        If (\_SB.PCI0.PEG0.PEGP.OPTF)
                        {
                            \_SB.PCI0.PEG0.PEGP.HDAE = One
                        }
                        Else
                        {
                            \_SB.PCI0.PEG0.PEGP.HDAE = Zero
                        }
                    }

                    _STA = One
                }
            }

            Method (_OFF, 0, Serialized)  // _OFF: Power Off
            {
                If ((\_SB.PCI0.PEG0.PEGP.TDGC == One))
                {
                    CreateField (\_SB.PCI0.PEG0.PEGP.TGPC, Zero, 0x03, GUPC)
                    If ((ToInteger (GUPC) == One))
                    {
                        \_SB.PCI0.PEG0.PEGP.GC6I ()
                    }
                    ElseIf ((ToInteger (GUPC) == 0x02))
                    {
                        \_SB.PCI0.PEG0.PEGP.GC6I ()
                    }

                    _STA = Zero
                }
                Else
                {
                    PGOF (Zero)
                    _STA = Zero
                }
            }
        }

        Name (_PR0, Package (0x01)  // _PR0: Power Resources for D0
        {
            PG00
        })
        Name (_PR2, Package (0x01)  // _PR2: Power Resources for D2
        {
            PG00
        })
        Name (_PR3, Package (0x01)  // _PR3: Power Resources for D3hot
        {
            PG00
        })
    }

    Scope (\_SB.PCI0)
    {
        Method (PGON, 1, Serialized)
        {
            PION = Arg0
            If ((PION == Zero))
            {
                If ((SGGP == Zero))
                {
                    Return (Zero)
                }
            }
            ElseIf ((PION == One))
            {
                If ((P1GP == Zero))
                {
                    Return (Zero)
                }
            }
            ElseIf ((PION == 0x02))
            {
                If ((P2GP == Zero))
                {
                    Return (Zero)
                }
            }

            PEBA = \XBAS /* External reference */
            PDEV = GDEV (PION)
            PFUN = GFUN (PION)
            If (CondRefOf (SCLK))
            {
                SPCO (SCLK, One)
            }

            If ((CCHK (PION, One) == Zero))
            {
                Return (Zero)
            }

            GPPR (PION, One)
            If (\_OSI ("Linux-Dell-Video"))
            {
                If ((PION == Zero))
                {
                    P0AP = Zero
                    P0RM = Zero
                }
                ElseIf ((PION == One))
                {
                    P1AP = Zero
                    P1RM = Zero
                }
                ElseIf ((PION == 0x02))
                {
                    P2AP = Zero
                    P2RM = Zero
                }

                If ((PION == Zero))
                {
                    P0LD = Zero
                    P0TR = One
                    TCNT = Zero
                    While ((TCNT < LDLY))
                    {
                        If ((P0VC == Zero))
                        {
                            Break
                        }

                        Sleep (0x10)
                        TCNT += 0x10
                    }
                }
                ElseIf ((PION == One))
                {
                    P1LD = Zero
                    P1TR = One
                    TCNT = Zero
                    While ((TCNT < LDLY))
                    {
                        If ((P1VC == Zero))
                        {
                            Break
                        }

                        Sleep (0x10)
                        TCNT += 0x10
                    }

                    If ((PION == 0x02))
                    {
                        P2LD = Zero
                        P2TR = One
                        TCNT = Zero
                        While ((TCNT < LDLY))
                        {
                            If ((P2VC == Zero))
                            {
                                Break
                            }

                            Sleep (0x10)
                            TCNT += 0x10
                        }
                    }
                }
            }
            Else
            {
                RTEN (PION)
            }

            If ((PBGE != Zero))
            {
                If (SBDL (PION))
                {
                    PUAB (PION)
                    CBDL = GUBC (PION)
                    MBDL = GMXB (PION)
                    If ((CBDL > MBDL))
                    {
                        CBDL = MBDL /* \_SB_.PCI0.MBDL */
                    }

                    PDUB (PION, CBDL)
                }
            }

            \_SB.PCI0.PEG0.LREN = \_SB.PCI0.PEG0.PEGP.LTRE
            \_SB.PCI0.PEG0.CEDR = One
            If ((PION == Zero))
            {
                LCT0 = ((ELC0 & 0x43) | (LCT0 & 0xFFBC))
            }
            ElseIf ((PION == One))
            {
                S1VI = H1VI /* \_SB_.PCI0.H1VI */
                S1DI = H1DI /* \_SB_.PCI0.H1DI */
                LCT1 = ((ELC1 & 0x43) | (LCT1 & 0xFFBC))
            }
            ElseIf ((PION == 0x02))
            {
                S2VI = H2VI /* \_SB_.PCI0.H2VI */
                S2DI = H2DI /* \_SB_.PCI0.H2DI */
                LCT2 = ((ELC2 & 0x43) | (LCT2 & 0xFFBC))
            }

            SGPO (SGGP, Zero, 0x03010004, One, One)
            Return (Zero)
        }

        Method (PGOF, 1, Serialized)
        {
            SGPO (SGGP, Zero, 0x03010004, One, Zero)
            PIOF = Arg0
            If ((PIOF == Zero))
            {
                If ((SGGP == Zero))
                {
                    Return (Zero)
                }
            }
            ElseIf ((PIOF == One))
            {
                If ((P1GP == Zero))
                {
                    Return (Zero)
                }
            }
            ElseIf ((PIOF == 0x02))
            {
                If ((P2GP == Zero))
                {
                    Return (Zero)
                }
            }

            PEBA = \XBAS /* External reference */
            PDEV = GDEV (PIOF)
            PFUN = GFUN (PIOF)
            If ((CCHK (PIOF, Zero) == Zero))
            {
                Return (Zero)
            }

            \_SB.PCI0.PEG0.PEGP.LTRE = \_SB.PCI0.PEG0.LREN
            If ((Arg0 == Zero))
            {
                ELC0 = LCT0 /* \_SB_.PCI0.LCT0 */
                H0VI = S0VI /* \_SB_.PCI0.S0VI */
                H0DI = S0DI /* \_SB_.PCI0.S0DI */
                ECP0 = LCP0 /* \_SB_.PCI0.LCP0 */
            }
            ElseIf ((Arg0 == One))
            {
                ELC1 = LCT1 /* \_SB_.PCI0.LCT1 */
                H1VI = S1VI /* \_SB_.PCI0.S1VI */
                H1DI = S1DI /* \_SB_.PCI0.S1DI */
                ECP1 = LCP1 /* \_SB_.PCI0.LCP1 */
            }
            ElseIf ((Arg0 == 0x02))
            {
                ELC2 = LCT2 /* \_SB_.PCI0.LCT2 */
                H2VI = S2VI /* \_SB_.PCI0.S2VI */
                H2DI = S2DI /* \_SB_.PCI0.S2DI */
                ECP2 = LCP2 /* \_SB_.PCI0.LCP2 */
            }

            If (\_OSI ("Linux-Dell-Video"))
            {
                If ((PIOF == Zero))
                {
                    P0LD = One
                    TCNT = Zero
                    While ((TCNT < LDLY))
                    {
                        If ((P0LT == 0x08))
                        {
                            Break
                        }

                        Sleep (0x10)
                        TCNT += 0x10
                    }

                    P0RM = One
                    P0AP = 0x03
                }
                ElseIf ((PIOF == One))
                {
                    P1LD = One
                    TCNT = Zero
                    While ((TCNT < LDLY))
                    {
                        If ((P1LT == 0x08))
                        {
                            Break
                        }

                        Sleep (0x10)
                        TCNT += 0x10
                    }

                    P1RM = One
                    P1AP = 0x03
                }
                ElseIf ((PIOF == 0x02))
                {
                    P2LD = One
                    TCNT = Zero
                    While ((TCNT < LDLY))
                    {
                        If ((P2LT == 0x08))
                        {
                            Break
                        }

                        Sleep (0x10)
                        TCNT += 0x10
                    }

                    P2RM = One
                    P2AP = 0x03
                }
            }
            Else
            {
                RTDS (PIOF)
            }

            If ((PBGE != Zero))
            {
                If (SBDL (PIOF))
                {
                    MBDL = GMXB (PIOF)
                    PDUB (PIOF, MBDL)
                }
            }

            If (CondRefOf (SCLK))
            {
                SPCO (SCLK, Zero)
            }

            If ((Arg0 == Zero))
            {
                Divide (\_SB.PCI0.PEG0.LNRD, 0x03E8, Local0, Local1)
                Local0 = Zero
                While ((Local0 < (Local1 * 0x0A)))
                {
                    Stall (0x64)
                    Local0++
                }
            }
            ElseIf ((Arg0 == One))
            {
                Divide (\_SB.PCI0.PEG1.LNRD, 0x03E8, Local0, Local1)
                Sleep (Local1)
            }
            ElseIf ((Arg0 == 0x02))
            {
                Divide (\_SB.PCI0.PEG2.LNRD, 0x03E8, Local0, Local1)
                Sleep (Local1)
            }

            GPPR (PIOF, Zero)
            If (!\_OSI ("Linux-Dell-Video"))
            {
                DIWK (PIOF)
            }

            Return (Zero)
        }
    }

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        Name (LTRE, Zero)
        Method (_STA, 0, NotSerialized)  // _STA: Status
        {
            Return (0x0F)
        }

        OperationRegion (PCIM, SystemMemory, (\XBAS + (\_SB.PCI0.PEG0.PRBN << 0x14)), 0x0600)
        Field (PCIM, DWordAcc, NoLock, Preserve)
        {
            NVID,   16, 
            NDID,   16, 
            CMDR,   8, 
            VGAR,   2008, 
            Offset (0x48B), 
                ,   1, 
            HDAE,   1
        }

        OperationRegion (DGPU, SystemMemory, (\XBAS + (\_SB.PCI0.PEG0.PRBN << 0x14)), 0x44)
        Field (DGPU, DWordAcc, NoLock, Preserve)
        {
            Offset (0x40), 
            SSSV,   32
        }

        Name (NGEN, Zero)
        Method (_INI, 0, NotSerialized)  // _INI: Initialize
        {
            \_SB.PCI0.PEG0.PEGP._ADR = Zero
        }

        Method (CKNG, 0, Serialized)
        {
            If ((NGEN != Zero))
            {
                Return (NGEN) /* \_SB_.PCI0.PEG0.PEGP.NGEN */
            }

            Switch (ToInteger (NDID))
            {
                Case (Package (0x03)
                    {
                        0x1C8D, 
                        0x1C91, 
                        0x1D13
                    }

)
                {
                    NGEN = 0x11
                }
                Case (Package (0x0C)
                    {
                        0x1E90, 
                        0x1E91, 
                        0x1F10, 
                        0x1F11, 
                        0x1F12, 
                        0x1F14, 
                        0x1F15, 
                        0x1F36, 
                        0x1F94, 
                        0x1F95, 
                        0x2191, 
                        0x1F99
                    }

)
                {
                    NGEN = 0x12
                }
                Case (Package (0x02)
                    {
                        0x1FB8, 
                        0x1FB9
                    }

)
                {
                    NGEN = 0x13
                }
                Default
                {
                    NGEN = 0x11
                }

            }

            Return (NGEN) /* \_SB_.PCI0.PEG0.PEGP.NGEN */
        }

        Method (_ROM, 2, NotSerialized)  // _ROM: Read-Only Memory
        {
            Local0 = Arg0
            Local1 = Arg1
            If ((Local1 > 0x1000))
            {
                Local1 = 0x1000
            }

            If ((Local0 > RVBS))
            {
                Return (Buffer (Local1)
                {
                     0x00                                             // .
                })
            }

            Local3 = (Local1 * 0x08)
            Name (ROM1, Buffer (0x8000)
            {
                 0x00                                             // .
            })
            Name (ROM2, Buffer (Local1)
            {
                 0x00                                             // .
            })
            If ((Local0 < 0x8000))
            {
                ROM1 = VBS1 /* \_SB_.PCI0.PEG0.PEGP.VBS1 */
            }
            ElseIf ((Local0 < 0x00010000))
            {
                Local0 -= 0x8000
                ROM1 = VBS2 /* \_SB_.PCI0.PEG0.PEGP.VBS2 */
            }
            ElseIf ((Local0 < 0x00018000))
            {
                Local0 -= 0x00010000
                ROM1 = VBS3 /* \_SB_.PCI0.PEG0.PEGP.VBS3 */
            }
            ElseIf ((Local0 < 0x00020000))
            {
                Local0 -= 0x00018000
                ROM1 = VBS4 /* \_SB_.PCI0.PEG0.PEGP.VBS4 */
            }
            ElseIf ((Local0 < 0x00028000))
            {
                Local0 -= 0x00020000
                ROM1 = VBS5 /* \_SB_.PCI0.PEG0.PEGP.VBS5 */
            }
            ElseIf ((Local0 < 0x00030000))
            {
                Local0 -= 0x00028000
                ROM1 = VBS6 /* \_SB_.PCI0.PEG0.PEGP.VBS6 */
            }
            ElseIf ((Local0 < 0x00038000))
            {
                Local0 -= 0x00030000
                ROM1 = VBS7 /* \_SB_.PCI0.PEG0.PEGP.VBS7 */
            }
            ElseIf ((Local0 < 0x00040000))
            {
                Local0 -= 0x00038000
                ROM1 = VBS8 /* \_SB_.PCI0.PEG0.PEGP.VBS8 */
            }

            Local2 = (Local0 * 0x08)
            CreateField (ROM1, Local2, Local3, TMPB)
            ROM2 = TMPB /* \_SB_.PCI0.PEG0.PEGP._ROM.TMPB */
            Return (ROM2) /* \_SB_.PCI0.PEG0.PEGP._ROM.ROM2 */
        }

        Method (_DSM, 4, Serialized)  // _DSM: Device-Specific Method
        {
            If ((Arg0 == ToUUID ("a486d8f8-0bda-471b-a72b-6042a6b5bee0") /* Unknown UUID */))
            {
                Return (NVOP (Arg0, Arg1, Arg2, Arg3))
            }
            ElseIf ((Arg0 == ToUUID ("a3132d01-8cda-49ba-a52e-bc9d46df6b81") /* Unknown UUID */))
            {
                Return (GPS (Arg0, Arg1, Arg2, Arg3))
            }
            ElseIf ((Arg0 == ToUUID ("cbeca351-067b-4924-9cbd-b46b00b86f34") /* Unknown UUID */))
            {
                Return (NVJT (Arg0, Arg1, Arg2, Arg3))
            }
            ElseIf ((Arg0 == ToUUID ("d4a50b75-65c7-46f7-bfb7-41514cea0244") /* Unknown UUID */))
            {
                Return (NBCI (Arg0, Arg1, Arg2, Arg3))
            }
            Else
            {
                Return (0x80000001)
            }
        }
    }

    Scope (\_SB.PCI0.PEG0)
    {
        Device (NXHC)
        {
            Name (_ADR, 0x02)  // _ADR: Address
            Device (NHUB)
            {
                Name (_ADR, Zero)  // _ADR: Address
                Device (SS01)
                {
                    Name (_ADR, One)  // _ADR: Address
                    Method (_UPC, 0, Serialized)  // _UPC: USB Port Capabilities
                    {
                        Name (NUPC, Package (0x04)
                        {
                            Zero, 
                            0xFF, 
                            Zero, 
                            Zero
                        })
                        Return (NUPC) /* \_SB_.PCI0.PEG0.NXHC.NHUB.SS01._UPC.NUPC */
                    }

                    Method (_PLD, 0, NotSerialized)  // _PLD: Physical Location of Device
                    {
                        Return (NPLD (Zero, One))
                    }
                }

                Device (SS02)
                {
                    Name (_ADR, 0x02)  // _ADR: Address
                    Method (_UPC, 0, Serialized)  // _UPC: USB Port Capabilities
                    {
                        Name (NUPC, Package (0x04)
                        {
                            Zero, 
                            0xFF, 
                            Zero, 
                            Zero
                        })
                        Return (NUPC) /* \_SB_.PCI0.PEG0.NXHC.NHUB.SS02._UPC.NUPC */
                    }

                    Method (_PLD, 0, NotSerialized)  // _PLD: Physical Location of Device
                    {
                        Return (NPLD (Zero, 0x02))
                    }
                }

                Device (SS03)
                {
                    Name (_ADR, 0x03)  // _ADR: Address
                    Method (_UPC, 0, Serialized)  // _UPC: USB Port Capabilities
                    {
                        Name (NUPC, Package (0x04)
                        {
                            Zero, 
                            0xFF, 
                            Zero, 
                            Zero
                        })
                        Return (NUPC) /* \_SB_.PCI0.PEG0.NXHC.NHUB.SS03._UPC.NUPC */
                    }

                    Method (_PLD, 0, NotSerialized)  // _PLD: Physical Location of Device
                    {
                        Return (NPLD (Zero, 0x03))
                    }
                }

                Device (SS04)
                {
                    Name (_ADR, 0x04)  // _ADR: Address
                    Method (_UPC, 0, Serialized)  // _UPC: USB Port Capabilities
                    {
                        Name (NUPC, Package (0x04)
                        {
                            Zero, 
                            0xFF, 
                            Zero, 
                            Zero
                        })
                        Return (NUPC) /* \_SB_.PCI0.PEG0.NXHC.NHUB.SS04._UPC.NUPC */
                    }

                    Method (_PLD, 0, NotSerialized)  // _PLD: Physical Location of Device
                    {
                        Return (NPLD (Zero, 0x04))
                    }
                }

                Device (SS05)
                {
                    Name (_ADR, 0x05)  // _ADR: Address
                    Method (_UPC, 0, Serialized)  // _UPC: USB Port Capabilities
                    {
                        Name (NUPC, Package (0x04)
                        {
                            Zero, 
                            0xFF, 
                            Zero, 
                            Zero
                        })
                        Return (NUPC) /* \_SB_.PCI0.PEG0.NXHC.NHUB.SS05._UPC.NUPC */
                    }

                    Method (_PLD, 0, NotSerialized)  // _PLD: Physical Location of Device
                    {
                        Return (NPLD (Zero, 0x05))
                    }
                }

                Device (SS06)
                {
                    Name (_ADR, 0x06)  // _ADR: Address
                    Method (_UPC, 0, Serialized)  // _UPC: USB Port Capabilities
                    {
                        Name (NUPC, Package (0x04)
                        {
                            Zero, 
                            0xFF, 
                            Zero, 
                            Zero
                        })
                        Return (NUPC) /* \_SB_.PCI0.PEG0.NXHC.NHUB.SS06._UPC.NUPC */
                    }

                    Method (_PLD, 0, NotSerialized)  // _PLD: Physical Location of Device
                    {
                        Return (NPLD (Zero, 0x06))
                    }
                }
            }

            Method (NPLD, 2, Serialized)
            {
                Name (PCKG, Package (0x01)
                {
                    Buffer (0x10){}
                })
                CreateField (DerefOf (PCKG [Zero]), Zero, 0x07, REV)
                REV = One
                CreateField (DerefOf (PCKG [Zero]), 0x40, One, VISI)
                VISI = Arg0
                CreateField (DerefOf (PCKG [Zero]), 0x57, 0x08, GPOS)
                GPOS = Arg1
                Return (PCKG) /* \_SB_.PCI0.PEG0.NXHC.NPLD.PCKG */
            }
        }
    }

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        Name (TDGC, Zero)
        Name (DGCX, Zero)
        Name (TGPC, Buffer (0x04)
        {
             0x00                                             // .
        })
        Name (GC6E, Zero)
        Method (NVJT, 4, Serialized)
        {
            Debug = "------- NV JT DSM --------"
            If ((Arg1 < 0x0100))
            {
                Return (0x80000001)
            }

            Switch (ToInteger (Arg2))
            {
                Case (Zero)
                {
                    Debug = "   JT fun0 JT_FUNC_SUPPORT"
                    Return (Buffer (0x04)
                    {
                         0x1B, 0x00, 0x00, 0x00                           // ....
                    })
                }
                Case (One)
                {
                    Debug = "   JT fun1 JT_FUNC_CAPS"
                    Name (JTCA, Buffer (0x04)
                    {
                         0x00                                             // .
                    })
                    CreateField (JTCA, Zero, One, JTEN)
                    CreateField (JTCA, One, 0x02, SREN)
                    CreateField (JTCA, 0x03, 0x02, PLPR)
                    CreateField (JTCA, 0x05, One, SRPR)
                    CreateField (JTCA, 0x06, 0x02, FBPR)
                    CreateField (JTCA, 0x08, 0x02, GUPR)
                    CreateField (JTCA, 0x0A, One, GC6R)
                    CreateField (JTCA, 0x0B, One, PTRH)
                    CreateField (JTCA, 0x0D, One, MHYB)
                    CreateField (JTCA, 0x0E, One, RPCL)
                    CreateField (JTCA, 0x0F, 0x02, GC6V)
                    CreateField (JTCA, 0x11, One, GEIS)
                    CreateField (JTCA, 0x12, One, GSWS)
                    CreateField (JTCA, 0x14, 0x0C, JTRV)
                    JTEN = One
                    GC6R = Zero
                    RPCL = One
                    SREN = One
                    FBPR = Zero
                    MHYB = One
                    If ((NGEN >= 0x12))
                    {
                        GC6V = 0x02
                        JTRV = 0x0200
                    }
                    Else
                    {
                        GUPR = Zero
                        PTRH = Zero
                        SREN = One
                        PLPR = 0x02
                        SRPR = Zero
                        JTRV = 0x0103
                    }

                    Return (JTCA) /* \_SB_.PCI0.PEG0.PEGP.NVJT.JTCA */
                }
                Case (0x02)
                {
                    Debug = "   JT fun2 JT_FUNC_POLICYSELECT"
                    Return (0x80000002)
                }
                Case (0x03)
                {
                    Debug = "   JT fun3 JT_FUNC_POWERCONTROL"
                    CreateField (Arg3, Zero, 0x03, GUPC)
                    CreateField (Arg3, 0x04, One, PLPC)
                    CreateField (Arg3, 0x07, One, ECOC)
                    CreateField (Arg3, 0x0E, 0x02, DFGC)
                    CreateField (Arg3, 0x10, 0x03, GPCX)
                    TGPC = Arg3
                    If (((ToInteger (GUPC) != Zero) || (ToInteger (DFGC
                        ) != Zero)))
                    {
                        TDGC = ToInteger (DFGC)
                        DGCX = ToInteger (GPCX)
                    }

                    Name (JTPC, Buffer (0x04)
                    {
                         0x00                                             // .
                    })
                    CreateField (JTPC, Zero, 0x03, GUPS)
                    CreateField (JTPC, 0x03, One, GPWO)
                    CreateField (JTPC, 0x07, One, PLST)
                    If ((ToInteger (DFGC) != Zero))
                    {
                        GPWO = One
                        GUPS = One
                        Return (JTPC) /* \_SB_.PCI0.PEG0.PEGP.NVJT.JTPC */
                    }

                    Debug = "   JT fun3 GUPC="
                    Debug = ToInteger (GUPC)
                    If ((ToInteger (GUPC) == One))
                    {
                        GC6I ()
                        PLST = One
                    }
                    ElseIf ((ToInteger (GUPC) == 0x02))
                    {
                        GC6I ()
                        If ((ToInteger (PLPC) == Zero))
                        {
                            PLST = Zero
                        }
                    }
                    ElseIf ((ToInteger (GUPC) == 0x03))
                    {
                        GC6O ()
                        If ((ToInteger (PLPC) != Zero))
                        {
                            PLST = Zero
                        }
                    }
                    ElseIf ((ToInteger (GUPC) == 0x04))
                    {
                        GC6O ()
                        If ((ToInteger (PLPC) != Zero))
                        {
                            PLST = Zero
                        }
                    }
                    ElseIf ((ToInteger (GUPC) == Zero))
                    {
                        GUPS = GETS ()
                        If ((ToInteger (GUPS) == One))
                        {
                            GPWO = One
                        }
                        Else
                        {
                            GPWO = Zero
                        }
                    }

                    Return (JTPC) /* \_SB_.PCI0.PEG0.PEGP.NVJT.JTPC */
                }
                Case (0x04)
                {
                    Debug = "   JT fun4 JT_FUNC_PLATPOLICY"
                    CreateField (Arg3, 0x02, One, PAUD)
                    CreateField (Arg3, 0x03, One, PADM)
                    CreateField (Arg3, 0x04, 0x04, PDGS)
                    Local0 = Zero
                    If ((NGEN <= 0x11))
                    {
                        If ((ToInteger (PADM) == One))
                        {
                            If ((ToInteger (PAUD) == Zero))
                            {
                                OPTF = Zero
                            }
                            Else
                            {
                                OPTF = One
                            }
                        }
                    }

                    Local0 = (\_SB.PCI0.PEG0.PEGP.HDAE << 0x02)
                    Return (Local0)
                }

            }

            Return (0x80000002)
        }

        Method (GC6I, 0, Serialized)
        {
            If ((NGEN >= 0x12))
            {
                Debug = "   JT GC6I"
                \_SB.PCI0.PEG0.PEGP.LTRE = \_SB.PCI0.PEG0.LREN
                \_SB.PCI0.RTDS (Zero)
                Sleep (0x0A)
                SGPO (SGGP, HRE0, HRG0, HRA0, One)
                If (CondRefOf (\_SB.PCI0.LPCB.ECDV.ECNV))
                {
                    \_SB.PCI0.LPCB.ECDV.ECNV (One)
                }
            }
            Else
            {
                \_SB.PCI0.PEG0.PEGP.LTRE = \_SB.PCI0.PEG0.LREN
                \_SB.PCI0.PEG0.LNKD = One
                While ((\_SB.PCI0.PEG0.LNKS != Zero))
                {
                    Sleep (One)
                }

                \_SB.PCI0.P0RM = One
                \_SB.PCI0.P0AP = 0x03
                While ((\_SB.GGIV (FBEN) != One))
                {
                    Sleep (One)
                }
            }

            GC6E = One
        }

        Method (GC6O, 0, Serialized)
        {
            Debug = "   JT GC6O"
            If ((NGEN >= 0x12))
            {
                SGPO (SGGP, HRE0, HRG0, HRA0, Zero)
                \_SB.PCI0.RTEN (Zero)
                \_SB.PCI0.PEG0.CMDR |= 0x04
                \_SB.PCI0.PEG0.LREN = \_SB.PCI0.PEG0.PEGP.LTRE
                \_SB.PCI0.PEG0.CEDR = One
                If (CondRefOf (\_SB.PCI0.LPCB.ECDV.ECNV))
                {
                    \_SB.PCI0.LPCB.ECDV.ECNV (Zero)
                }
            }
            Else
            {
                \_SB.PCI0.P0RM = Zero
                \_SB.PCI0.P0AP = Zero
                \_SB.PCI0.PEG0.LNKD = Zero
                If ((\_SB.GGIV (FBEN) == One))
                {
                    \_SB.SGOV (ENVT, Zero)
                    While ((\_SB.GGIV (FBEN) != Zero))
                    {
                        Sleep (One)
                    }

                    \_SB.SGOV (ENVT, One)
                }

                While ((\_SB.PCI0.PEG0.LNKS < 0x07))
                {
                    Sleep (One)
                }

                \_SB.PCI0.PEG0.LREN = \_SB.PCI0.PEG0.PEGP.LTRE
                \_SB.PCI0.PEG0.CEDR = One
            }

            GC6E = Zero
        }

        Method (GETS, 0, Serialized)
        {
            If ((GC6E == Zero))
            {
                Return (One)
            }
            Else
            {
                Return (0x03)
            }
        }
    }

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        Name (VGAB, Buffer (0xFB)
        {
             0x00                                             // .
        })
        Method (_PS0, 0, NotSerialized)  // _PS0: Power State 0
        {
            If ((DGPS != Zero))
            {
                \_SB.PCI0.PGON (Zero)
                If ((GPRF != One))
                {
                    VGAR = VGAB /* \_SB_.PCI0.PEG0.PEGP.VGAB */
                }

                DGPS = Zero
            }
        }

        Method (_PS3, 0, NotSerialized)  // _PS3: Power State 3
        {
            If ((OMPR == 0x03))
            {
                If ((GPRF != One))
                {
                    VGAB = VGAR /* \_SB_.PCI0.PEG0.PEGP.VGAR */
                }

                \_SB.PCI0.PGOF (Zero)
                DGPS = One
                OMPR = 0x02
            }
        }

        Name (DGPS, Zero)
        Name (OMPR, 0x02)
        Name (GPRF, Zero)
        Method (NVOP, 4, Serialized)
        {
            Debug = "------- NV OPTIMUS DSM --------"
            If ((Arg1 != 0x0100))
            {
                Return (0x80000001)
            }

            Switch (ToInteger (Arg2))
            {
                Case (Zero)
                {
                    Return (Buffer (0x04)
                    {
                         0x01, 0x00, 0x00, 0x0C                           // ....
                    })
                }
                Case (0x1A)
                {
                    CreateField (Arg3, Zero, One, FLCH)
                    CreateField (Arg3, One, One, DVSR)
                    CreateField (Arg3, 0x02, One, DVSC)
                    CreateField (Arg3, 0x18, 0x02, OPCE)
                    If ((ToInteger (FLCH) & (ToInteger (OPCE) != OMPR)))
                    {
                        OMPR = OPCE /* \_SB_.PCI0.PEG0.PEGP.NVOP.OPCE */
                    }

                    Local0 = Buffer (0x04)
                        {
                             0x00, 0x00, 0x00, 0x00                           // ....
                        }
                    CreateField (Local0, Zero, One, OPEN)
                    CreateField (Local0, 0x03, 0x02, CGCS)
                    CreateField (Local0, 0x06, One, SHPC)
                    CreateField (Local0, 0x08, One, SNSR)
                    CreateField (Local0, 0x18, 0x03, DGPC)
                    CreateField (Local0, 0x1B, 0x02, OHAC)
                    OPEN = One
                    SHPC = One
                    DGPC = One
                    OHAC = 0x03
                    If ((NGEN <= 0x11))
                    {
                        OHAC = 0x02
                    }

                    If (ToInteger (DVSC))
                    {
                        If (ToInteger (DVSR))
                        {
                            GPRF = One
                        }
                        Else
                        {
                            GPRF = Zero
                        }
                    }

                    SNSR = GPRF /* \_SB_.PCI0.PEG0.PEGP.GPRF */
                    If ((DGPS == Zero))
                    {
                        CGCS = 0x03
                    }
                    Else
                    {
                        CGCS = Zero
                    }

                    Return (Local0)
                }
                Case (0x1B)
                {
                    CreateField (Arg3, Zero, One, OACC)
                    CreateField (Arg3, One, One, UOAC)
                    CreateField (Arg3, 0x02, 0x08, OPDA)
                    CreateField (Arg3, 0x0A, One, OPDE)
                    Local1 = Zero
                    If ((NGEN >= 0x12)){}
                    ElseIf (ToInteger (UOAC))
                    {
                        OPTF = Zero
                        If (ToInteger (OACC))
                        {
                            OPTF = One
                        }
                    }

                    Return (Local1)
                }

            }

            Return (0x80000002)
        }
    }

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        Name (WGPU, 0x50)
        If (CondRefOf (\ODV0))
        {
            Switch (\ODV0)
            {
                Case (Zero)
                {
                    WGPU = 0x50
                }
                Case (One)
                {
                    WGPU = 0x4B
                }
                Case (0x02)
                {
                    WGPU = 0x4B
                }
                Case (0x03)
                {
                    WGPU = 0x57
                }

            }
        }

        Name (REQU, Zero)
        Name (PSAP, Zero)
        Name (NLIM, Zero)
        Name (PSLS, Zero)
        Name (PTGP, Zero)
        Name (TGPV, 0x2710)
        Name (CTGP, Zero)
        Name (GPBS, 0x24)
        If ((CKNG () >= 0x12))
        {
            GPBS = 0x28
        }

        Name (GPSP, Buffer (GPBS){})
        CreateDWordField (GPSP, Zero, RETN)
        CreateDWordField (GPSP, 0x04, VRV1)
        CreateDWordField (GPSP, 0x08, TGPU)
        CreateDWordField (GPSP, 0x0C, PDTS)
        CreateDWordField (GPSP, 0x10, SFAN)
        CreateDWordField (GPSP, 0x14, SKNT)
        CreateDWordField (GPSP, 0x18, CPUE)
        CreateDWordField (GPSP, 0x1C, TMP1)
        CreateDWordField (GPSP, 0x20, TMP2)
        If ((CKNG () >= 0x12))
        {
            CreateDWordField (GPSP, 0x24, PCGP)
        }

        Method (GPS, 4, Serialized)
        {
            Debug = "------- NV GPS DSM --------"
            If ((Arg1 != 0x0100))
            {
                Return (0x80000002)
            }

            Local0 = ToInteger (Arg2)
            If ((Local0 == Zero))
            {
                Debug = "   GPS fun 0"
                If ((CKNG () >= 0x12))
                {
                    Return (Buffer (0x08)
                    {
                         0x01, 0x00, 0x08, 0x00, 0x01, 0x04, 0x00, 0x00   // ........
                    })
                }
                Else
                {
                    Return (Buffer (0x08)
                    {
                         0x01, 0x00, 0x08, 0x00, 0x0F, 0x04, 0x00, 0x00   // ........
                    })
                }
            }
            ElseIf ((Local0 == 0x13))
            {
                Debug = "   GPS fun 19"
                \_SB.PCI0.PEG0.PEGP.HGPS (Arg3)
                Name (RET9, Zero)
                RET9 |= 0x04
                Return (RET9) /* \_SB_.PCI0.PEG0.PEGP.GPS_.RET9 */
            }
            ElseIf ((Local0 == 0x20))
            {
                Debug = "   GPS fun 32"
                Name (RET1, Zero)
                CreateBitField (Arg3, 0x02, SPBI)
                If ((CKNG () <= 0x11))
                {
                    CreateBitField (Arg3, 0x18, NRIT)
                    CreateBitField (Arg3, 0x19, NRIS)
                    If (NRIS)
                    {
                        If (NRIT)
                        {
                            RET1 |= 0x01000000
                        }
                        Else
                        {
                            RET1 &= 0xFEFFFFFF
                        }
                    }

                    RET1 |= 0x40000000
                    Debug = "== GPS: HWPV =="
                    Debug = \_SB.HWPV /* External reference */
                    If ((\_SB.HWPV & 0x02))
                    {
                        RET1 |= 0x00800000
                    }
                }

                If (NLIM)
                {
                    RET1 |= One
                }

                PSLS = One
                If (PSLS)
                {
                    RET1 |= 0x02
                }

                If ((CKNG () >= 0x12))
                {
                    If (PTGP)
                    {
                        RET1 |= 0x00100000
                    }
                }

                If (CTGP)
                {
                    RET1 |= 0x00400000
                }

                If ((REQU == One))
                {
                    RET1 |= One
                    REQU = Zero
                }

                Return (RET1) /* \_SB_.PCI0.PEG0.PEGP.GPS_.RET1 */
            }
            ElseIf ((Local0 == 0x2A))
            {
                Debug = "   GPS fun 42"
                CreateField (Arg3, Zero, 0x04, PSH0)
                CreateBitField (Arg3, 0x08, GPUT)
                CreateBitField (Arg3, 0x09, CPUT)
                VRV1 = 0x00010000
                If ((CKNG () >= 0x12))
                {
                    PCGP = TGPV /* \_SB_.PCI0.PEG0.PEGP.TGPV */
                }

                Switch (ToInteger (PSH0))
                {
                    Case (Zero)
                    {
                        If ((CKNG () <= 0x11))
                        {
                            If (CPUT)
                            {
                                RETN = 0x0200
                                RETN |= ToInteger (PSH0)
                            }
                        }

                        Return (GPSP) /* \_SB_.PCI0.PEG0.PEGP.GPSP */
                    }
                    Case (One)
                    {
                        If ((CKNG () <= 0x11))
                        {
                            RETN = 0x0300
                            PDTS = 0x03E8
                        }
                        Else
                        {
                            RETN = 0x0100
                        }

                        RETN |= ToInteger (PSH0)
                        If ((CKNG () >= 0x12))
                        {
                            If (PTGP)
                            {
                                RETN |= 0x8000
                            }
                        }

                        Return (GPSP) /* \_SB_.PCI0.PEG0.PEGP.GPSP */
                    }
                    Case (0x02)
                    {
                        RETN = 0x0102
                        TGPU = WGPU /* \_SB_.PCI0.PEG0.PEGP.WGPU */
                        If ((CKNG () <= 0x11))
                        {
                            PDTS = Zero
                            SFAN = Zero
                            CPUE = Zero
                            SKNT = Zero
                            TMP1 = Zero
                            TMP2 = Zero
                        }

                        Sleep (0x0A)
                        If ((CKNG () >= 0x12))
                        {
                            If (PTGP)
                            {
                                RETN |= 0x8000
                            }
                        }

                        Return (GPSP) /* \_SB_.PCI0.PEG0.PEGP.GPSP */
                    }

                }
            }
            ElseIf ((Local0 == 0x21))
            {
                Debug = "   GPS fun 33"
                Return (\_SB.PR00._PSS ())
            }
            ElseIf ((Local0 == 0x22))
            {
                Debug = "   GPS fun 34"
                CreateByteField (Arg3, Zero, PCAP)
                \_SB.CPPC = PCAP /* \_SB_.PCI0.PEG0.PEGP.GPS_.PCAP */
                PNOT ()
                PSAP = PCAP /* \_SB_.PCI0.PEG0.PEGP.GPS_.PCAP */
                Return (PCAP) /* \_SB_.PCI0.PEG0.PEGP.GPS_.PCAP */
            }
            ElseIf ((Local0 == 0x23))
            {
                Debug = "   GPS fun 35"
                Return (PSAP) /* \_SB_.PCI0.PEG0.PEGP.PSAP */
            }
            Else
            {
                Return (0x80000002)
            }

            Return (0x80000002)
        }
    }

    Scope (\_SB.PCI0.PEG0.PEGP)
    {
        Name (GSV1, Buffer (One)
        {
             0x00                                             // .
        })
        Name (GSV2, Buffer (One)
        {
             0x00                                             // .
        })
        Name (GSDR, Buffer (0xA1)
        {
            /* 0000 */  0x57, 0x74, 0xDC, 0x86, 0x75, 0x84, 0xEC, 0xE7,  // Wt..u...
            /* 0008 */  0x52, 0x44, 0xA1, 0x00, 0x00, 0x00, 0x00, 0x01,  // RD......
            /* 0010 */  0x00, 0x00, 0x00, 0x00, 0xDE, 0x10, 0x00, 0x00,  // ........
            /* 0018 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0020 */  0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x34, 0x00,  // ......4.
            /* 0028 */  0x00, 0x00, 0x01, 0x00, 0x47, 0x00, 0x00, 0x00,  // ....G...
            /* 0030 */  0x02, 0x00, 0x45, 0x00, 0x00, 0x00, 0x03, 0x00,  // ..E.....
            /* 0038 */  0x51, 0x00, 0x00, 0x00, 0x04, 0x00, 0x4F, 0x00,  // Q.....O.
            /* 0040 */  0x00, 0x00, 0x05, 0x00, 0x4D, 0x00, 0x00, 0x00,  // ....M...
            /* 0048 */  0x06, 0x00, 0x4B, 0x00, 0x00, 0x00, 0x07, 0x00,  // ..K.....
            /* 0050 */  0x49, 0x00, 0x00, 0x00, 0x08, 0x00, 0x47, 0x00,  // I.....G.
            /* 0058 */  0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xD9, 0x1C,  // ........
            /* 0060 */  0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,  // ........
            /* 0068 */  0x41, 0x5D, 0xC9, 0x00, 0x01, 0x24, 0x2E, 0x00,  // A]...$..
            /* 0070 */  0x02, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x01,  // ........
            /* 0078 */  0x00, 0x00, 0x00, 0xD9, 0x1C, 0x04, 0x00, 0x00,  // ........
            /* 0080 */  0x00, 0x01, 0x00, 0x00, 0x00, 0x60, 0x68, 0x9E,  // .....`h.
            /* 0088 */  0x35, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // 5.......
            /* 0090 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0098 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 00A0 */  0x00                                             // .
        })
        Method (NBCI, 4, Serialized)
        {
            Debug = "------- NV NBCI DSM --------"
            If ((Arg1 != 0x0102))
            {
                Debug = " NBCI DSM: NOT SUPPORTED!"
                Return (0x80000002)
            }

            If ((Arg2 == Zero))
            {
                Return (Buffer (0x04)
                {
                     0x01, 0x00, 0x11, 0x00                           // ....
                })
            }

            If ((Arg2 == One))
            {
                Name (TEMP, Buffer (0x04)
                {
                     0x00, 0x00, 0x00, 0x00                           // ....
                })
                CreateDWordField (TEMP, Zero, STS0)
                STS0 |= Zero
                Return (TEMP) /* \_SB_.PCI0.PEG0.PEGP.NBCI.TEMP */
            }

            If ((Arg2 == 0x10))
            {
                CreateWordField (Arg3, 0x02, BFF0)
                If ((BFF0 == 0x564B))
                {
                    Return (GSV1) /* \_SB_.PCI0.PEG0.PEGP.GSV1 */
                }

                If ((BFF0 == 0x4452))
                {
                    Return (GSDR) /* \_SB_.PCI0.PEG0.PEGP.GSDR */
                }
            }

            If ((Arg2 == 0x14))
            {
                Return (Package (0x20)
                {
                    0x8001A450, 
                    0x0200, 
                    Zero, 
                    Zero, 
                    0x05, 
                    One, 
                    0xC8, 
                    0x32, 
                    0x03E8, 
                    0x0B, 
                    0x32, 
                    0x64, 
                    0x96, 
                    0xC8, 
                    0x012C, 
                    0x0190, 
                    0x01FE, 
                    0x0276, 
                    0x02F8, 
                    0x0366, 
                    0x03E8, 
                    Zero, 
                    0x64, 
                    0xC8, 
                    0x012C, 
                    0x0190, 
                    0x01F4, 
                    0x0258, 
                    0x02BC, 
                    0x0320, 
                    0x0384, 
                    0x03E8
                })
            }
        }
    }
}

