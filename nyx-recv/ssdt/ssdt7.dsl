/*
 * Intel ACPI Component Architecture
 * AML/ASL+ Disassembler version 20230628 (64-bit version)
 * Copyright (c) 2000 - 2023 Intel Corporation
 * 
 * Disassembling to symbolic ASL+ operators
 *
 * Disassembly of ssdt7.dat, Sat Sep  5 08:00:22 2026
 *
 * Original Table Header:
 *     Signature        "SSDT"
 *     Length           0x0000461D (17949)
 *     Revision         0x02
 *     Checksum         0xD5
 *     OEM ID           "INTEL "
 *     OEM Table ID     "DptfTabl"
 *     OEM Revision     0x00001000 (4096)
 *     Compiler ID      "INTL"
 *     Compiler Version 0x20160527 (538314023)
 */
DefinitionBlock ("", "SSDT", 2, "INTEL ", "DptfTabl", 0x00001000)
{
    External (_SB_.AAC0, FieldUnitObj)
    External (_SB_.ACRT, FieldUnitObj)
    External (_SB_.APSV, FieldUnitObj)
    External (_SB_.CBMI, FieldUnitObj)
    External (_SB_.CFGD, FieldUnitObj)
    External (_SB_.CLVL, FieldUnitObj)
    External (_SB_.CPPC, FieldUnitObj)
    External (_SB_.CTC0, FieldUnitObj)
    External (_SB_.CTC1, FieldUnitObj)
    External (_SB_.CTC2, FieldUnitObj)
    External (_SB_.OSCP, IntObj)
    External (_SB_.PAGD, DeviceObj)
    External (_SB_.PAGD._PUR, PkgObj)
    External (_SB_.PAGD._STA, MethodObj)    // 0 Arguments
    External (_SB_.PCI0, DeviceObj)
    External (_SB_.PCI0.B0D4, DeviceObj)
    External (_SB_.PCI0.LPCB.ECDV, DeviceObj)
    External (_SB_.PCI0.LPCB.ECDV.ECR1, MethodObj)    // 1 Arguments
    External (_SB_.PCI0.LPCB.ECDV.ECW1, MethodObj)    // 2 Arguments
    External (_SB_.PCI0.MHBR, FieldUnitObj)
    External (_SB_.PL10, FieldUnitObj)
    External (_SB_.PL11, FieldUnitObj)
    External (_SB_.PL12, FieldUnitObj)
    External (_SB_.PL20, FieldUnitObj)
    External (_SB_.PL21, FieldUnitObj)
    External (_SB_.PL22, FieldUnitObj)
    External (_SB_.PLW0, FieldUnitObj)
    External (_SB_.PLW1, FieldUnitObj)
    External (_SB_.PLW2, FieldUnitObj)
    External (_SB_.PR00, ProcessorObj)
    External (_SB_.PR00._PSS, MethodObj)    // 0 Arguments
    External (_SB_.PR00._TPC, IntObj)
    External (_SB_.PR00._TSD, MethodObj)    // 0 Arguments
    External (_SB_.PR00._TSS, MethodObj)    // 0 Arguments
    External (_SB_.PR00.LPSS, PkgObj)
    External (_SB_.PR00.TPSS, PkgObj)
    External (_SB_.PR00.TSMC, PkgObj)
    External (_SB_.PR00.TSMF, PkgObj)
    External (_SB_.PR01, ProcessorObj)
    External (_SB_.PR02, ProcessorObj)
    External (_SB_.PR03, ProcessorObj)
    External (_SB_.PR04, ProcessorObj)
    External (_SB_.PR05, ProcessorObj)
    External (_SB_.PR06, ProcessorObj)
    External (_SB_.PR07, ProcessorObj)
    External (_SB_.PR08, ProcessorObj)
    External (_SB_.PR09, ProcessorObj)
    External (_SB_.PR10, ProcessorObj)
    External (_SB_.PR11, ProcessorObj)
    External (_SB_.PR12, ProcessorObj)
    External (_SB_.PR13, ProcessorObj)
    External (_SB_.PR14, ProcessorObj)
    External (_SB_.PR15, ProcessorObj)
    External (_SB_.PR16, ProcessorObj)
    External (_SB_.PR17, ProcessorObj)
    External (_SB_.PR18, ProcessorObj)
    External (_SB_.PR19, ProcessorObj)
    External (_SB_.SLPB, DeviceObj)
    External (_SB_.TAR0, FieldUnitObj)
    External (_SB_.TAR1, FieldUnitObj)
    External (_SB_.TAR2, FieldUnitObj)
    External (_TZ_.ETMD, IntObj)
    External (_TZ_.TZ00, ThermalZoneObj)
    External (_TZ_.TZ01, ThermalZoneObj)
    External (ACTT, IntObj)
    External (ADBG, MethodObj)    // 1 Arguments
    External (ATMC, IntObj)
    External (ATPC, IntObj)
    External (BATR, IntObj)
    External (BMID, UnknownObj)
    External (CA2D, IntObj)
    External (CHGE, IntObj)
    External (CPUS, IntObj)
    External (CRTT, IntObj)
    External (CTDP, IntObj)
    External (DCFE, IntObj)
    External (DDDR, IntObj)
    External (DISE, IntObj)
    External (DISP, MethodObj)    // 1 Arguments
    External (DPHL, IntObj)
    External (DPLL, IntObj)
    External (DPTF, IntObj)
    External (ECRD, IntObj)
    External (FND1, IntObj)
    External (HIDW, MethodObj)    // 4 Arguments
    External (HIWC, MethodObj)    // 1 Arguments
    External (LPER, IntObj)
    External (LPOE, IntObj)
    External (LPOP, IntObj)
    External (LPOS, IntObj)
    External (LPOW, IntObj)
    External (MPL0, IntObj)
    External (MPL1, IntObj)
    External (MPL2, IntObj)
    External (ODV0, IntObj)
    External (ODV1, IntObj)
    External (ODV2, IntObj)
    External (ODV3, IntObj)
    External (ODV4, IntObj)
    External (ODV5, IntObj)
    External (PC00, IntObj)
    External (PLID, UnknownObj)
    External (PNHM, IntObj)
    External (PPPR, IntObj)
    External (PPSZ, IntObj)
    External (PSVT, IntObj)
    External (PTMC, IntObj)
    External (PTPC, IntObj)
    External (PWRE, IntObj)
    External (PWRS, IntObj)
    External (S2AT, IntObj)
    External (S2CT, IntObj)
    External (S2DE, IntObj)
    External (S2HT, IntObj)
    External (S2PT, IntObj)
    External (S2S3, IntObj)
    External (S3AT, IntObj)
    External (S3CT, IntObj)
    External (S3DE, IntObj)
    External (S3HT, IntObj)
    External (S3PT, IntObj)
    External (S3S3, IntObj)
    External (S4AT, IntObj)
    External (S4CT, IntObj)
    External (S4DE, IntObj)
    External (S4HT, IntObj)
    External (S4PT, IntObj)
    External (S4S3, IntObj)
    External (S5AT, IntObj)
    External (S5CT, IntObj)
    External (S5DE, IntObj)
    External (S5HT, IntObj)
    External (S5PT, IntObj)
    External (S5S3, IntObj)
    External (SAC3, IntObj)
    External (SACT, IntObj)
    External (SADE, IntObj)
    External (SAHT, IntObj)
    External (SAT1, IntObj)
    External (SAT2, IntObj)
    External (SC31, IntObj)
    External (SC32, IntObj)
    External (SCT1, IntObj)
    External (SCT2, IntObj)
    External (SGE1, IntObj)
    External (SGE2, IntObj)
    External (SHT1, IntObj)
    External (SHT2, IntObj)
    External (SPT1, IntObj)
    External (SPT2, IntObj)
    External (SSP2, IntObj)
    External (SSP3, IntObj)
    External (SSP4, IntObj)
    External (SSP5, IntObj)
    External (TCNT, IntObj)
    External (TJMX, IntObj)
    External (TSOD, IntObj)
    External (V1AT, IntObj)
    External (V1C3, IntObj)
    External (V1CR, IntObj)
    External (V1HT, IntObj)
    External (V1PV, IntObj)
    External (V2AT, IntObj)
    External (V2C3, IntObj)
    External (V2CR, IntObj)
    External (V2HT, IntObj)
    External (V2PV, IntObj)
    External (VSP1, IntObj)
    External (VSP2, IntObj)
    External (WAND, IntObj)
    External (WLC3, IntObj)
    External (WRAT, IntObj)
    External (WRCT, IntObj)
    External (WRFD, IntObj)
    External (WRHT, IntObj)
    External (WRPT, IntObj)
    External (WTSP, IntObj)
    External (WWAT, IntObj)
    External (WWC3, IntObj)
    External (WWCT, IntObj)
    External (WWHT, IntObj)
    External (WWPT, IntObj)

    Scope (\_SB.PCI0.LPCB.ECDV)
    {
        Method (DPST, 1, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x32, Arg0)
            Local0 = \_SB.PCI0.LPCB.ECDV.ECR1 (0x32)
            Return (Local0)
        }

        Method (DPRT, 0, NotSerialized)
        {
            Local0 = \_SB.PCI0.LPCB.ECDV.ECR1 (0x32)
            Return (Local0)
        }

        Method (KDRT, 1, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x33, Arg0)
            Local0 = \_SB.PCI0.LPCB.ECDV.ECR1 (0x34)
            If ((Local0 >= 0x80))
            {
                Local0 = Zero
            }

            Return (Local0)
        }

        Method (DSTL, 2, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x33, Arg0)
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x35, Arg1)
        }

        Method (DRTL, 1, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x33, Arg0)
            Local0 = \_SB.PCI0.LPCB.ECDV.ECR1 (0x35)
            Return (Local0)
        }

        Method (DSTH, 2, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x33, Arg0)
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x36, Arg1)
        }

        Method (DRTH, 1, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x33, Arg0)
            Local0 = \_SB.PCI0.LPCB.ECDV.ECR1 (0x36)
            Return (Local0)
        }

        Method (DSHY, 2, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x33, Arg0)
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x37, Arg1)
        }

        Method (DRHY, 1, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x33, Arg0)
            Local0 = \_SB.PCI0.LPCB.ECDV.ECR1 (0x37)
            Return (Local0)
        }

        Method (DSSQ, 1, NotSerialized)
        {
            \_SB.PCI0.LPCB.ECDV.ECW1 (0x38, Arg0)
        }

        Method (DSRQ, 0, NotSerialized)
        {
            Local0 = \_SB.PCI0.LPCB.ECDV.ECR1 (0x38)
            Return (Local0)
        }
    }

    Scope (\_SB)
    {
        Device (IETM)
        {
            Name (_HID, EisaId ("INT3400") /* Intel Dynamic Power Performance Management */)  // _HID: Hardware ID
            Method (_DSM, 4, Serialized)  // _DSM: Device-Specific Method
            {
                If (CondRefOf (HIWC))
                {
                    If (HIWC (Arg0))
                    {
                        If (CondRefOf (HIDW))
                        {
                            Return (HIDW (Arg0, Arg1, Arg2, Arg3))
                        }
                    }
                }

                Return (Buffer (One)
                {
                     0x00                                             // .
                })
            }

            Method (_STA, 0, NotSerialized)  // _STA: Status
            {
                If ((DPTF == One))
                {
                    If ((DDDR == One))
                    {
                        DISP ("EC_DPTF_STATE_ENABLE(1)\n")
                        \_SB.PCI0.LPCB.ECDV.DPST (One)
                        DDDR = One
                    }

                    DISP ("INTEL DPTF SUPPORTED\n")
                    Return (0x0F)
                }
                Else
                {
                    Return (Zero)
                }
            }

            Name (PTRP, Zero)
            Name (PSEM, Zero)
            Name (ATRP, Zero)
            Name (ASEM, Zero)
            Name (YTRP, Zero)
            Name (YSEM, Zero)
            Method (_OSC, 4, Serialized)  // _OSC: Operating System Capabilities
            {
                CreateDWordField (Arg3, Zero, STS1)
                CreateDWordField (Arg3, 0x04, CAP1)
                If ((Arg1 != One))
                {
                    STS1 &= 0xFFFFFF00
                    STS1 |= 0x0A
                    Return (Arg3)
                }

                If ((Arg2 != 0x02))
                {
                    STS1 &= 0xFFFFFF00
                    STS1 |= 0x02
                    Return (Arg3)
                }

                If (CondRefOf (\_SB.APSV))
                {
                    If ((PSEM == Zero))
                    {
                        PSEM = One
                        PTRP = \_SB.APSV /* External reference */
                    }
                }

                If (CondRefOf (\_SB.AAC0))
                {
                    If ((ASEM == Zero))
                    {
                        ASEM = One
                        ATRP = \_SB.AAC0 /* External reference */
                    }
                }

                If (CondRefOf (\_SB.ACRT))
                {
                    If ((YSEM == Zero))
                    {
                        YSEM = One
                        YTRP = \_SB.ACRT /* External reference */
                    }
                }

                If ((Arg0 == ToUUID ("b23ba85d-c8b7-3542-88de-8de2ffcfd698") /* Unknown UUID */))
                {
                    If (~(STS1 & One))
                    {
                        If ((CAP1 & One))
                        {
                            If ((CAP1 & 0x02))
                            {
                                \_SB.AAC0 = 0x6E
                                \_TZ.ETMD = Zero
                            }
                            Else
                            {
                                \_SB.AAC0 = ATRP /* \_SB_.IETM.ATRP */
                                \_TZ.ETMD = One
                            }

                            If ((CAP1 & 0x04))
                            {
                                \_SB.APSV = 0x6E
                            }
                            Else
                            {
                                \_SB.APSV = PTRP /* \_SB_.IETM.PTRP */
                            }

                            If ((CAP1 & 0x08))
                            {
                                \_SB.ACRT = 0xD2
                            }
                            Else
                            {
                                \_SB.ACRT = YTRP /* \_SB_.IETM.YTRP */
                            }
                        }
                        Else
                        {
                            \_SB.ACRT = YTRP /* \_SB_.IETM.YTRP */
                            \_SB.APSV = PTRP /* \_SB_.IETM.PTRP */
                            \_SB.AAC0 = ATRP /* \_SB_.IETM.ATRP */
                            \_TZ.ETMD = One
                        }

                        If (CondRefOf (\_TZ.TZ00))
                        {
                            Notify (\_TZ.TZ00, 0x81) // Information Change
                        }
                    }

                    Return (Arg3)
                }

                Return (Arg3)
            }

            Method (DCFG, 0, NotSerialized)
            {
                Return (\DCFE) /* External reference */
            }

            Name (ODVX, Package (0x06)
            {
                Zero, 
                Zero, 
                Zero, 
                Zero, 
                Zero, 
                Zero
            })
            Method (ODVP, 0, Serialized)
            {
                ODVX [Zero] = \ODV0 /* External reference */
                ODVX [One] = \ODV1 /* External reference */
                ODVX [0x02] = \ODV2 /* External reference */
                ODVX [0x03] = \ODV3 /* External reference */
                ODVX [0x04] = \ODV4 /* External reference */
                ODVX [0x05] = \ODV5 /* External reference */
                Return (ODVX) /* \_SB_.IETM.ODVX */
            }
        }
    }

    Scope (\_SB.PCI0.LPCB.ECDV)
    {
        Mutex (PATM, 0x00)
        Name (SNUM, Zero)
        Method (DPNT, 0, NotSerialized)
        {
            DISP ("DPNT Called\n")
        }
    }

    Scope (\_SB.IETM)
    {
        Method (KTOC, 1, Serialized)
        {
            If ((Arg0 > 0x0AAC))
            {
                Return (((Arg0 - 0x0AAC) / 0x0A))
            }
            Else
            {
                Return (Zero)
            }
        }

        Method (CTOK, 1, Serialized)
        {
            Return (((Arg0 * 0x0A) + 0x0AAC))
        }

        Method (C10K, 1, Serialized)
        {
            Name (TMP1, Buffer (0x10)
            {
                 0x00                                             // .
            })
            CreateByteField (TMP1, Zero, TMPL)
            CreateByteField (TMP1, One, TMPH)
            Local0 = (Arg0 + 0x0AAC)
            TMPL = (Local0 & 0xFF)
            TMPH = ((Local0 & 0xFF00) >> 0x08)
            ToInteger (TMP1, Local1)
            Return (Local1)
        }

        Method (K10C, 1, Serialized)
        {
            If ((Arg0 > 0x0AAC))
            {
                Return ((Arg0 - 0x0AAC))
            }
            Else
            {
                Return (Zero)
            }
        }
    }

    Scope (\_SB.PCI0.B0D4)
    {
        Name (PFLG, Zero)
        Method (_STA, 0, NotSerialized)  // _STA: Status
        {
            If ((\SADE == One))
            {
                Return (0x0F)
            }
            Else
            {
                Return (Zero)
            }
        }

        OperationRegion (MBAR, SystemMemory, ((MHBR << 0x0F) + 0x5000), 0x1000)
        Field (MBAR, ByteAcc, NoLock, Preserve)
        {
            Offset (0x930), 
            PTDP,   15, 
            Offset (0x932), 
            PMIN,   15, 
            Offset (0x934), 
            PMAX,   15, 
            Offset (0x936), 
            TMAX,   7, 
            Offset (0x938), 
            PWRU,   4, 
            Offset (0x939), 
            EGYU,   5, 
            Offset (0x93A), 
            TIMU,   4, 
            Offset (0x958), 
            Offset (0x95C), 
            LPMS,   1, 
            CTNL,   2, 
            Offset (0x978), 
            PCTP,   8, 
            Offset (0x998), 
            RP0C,   8, 
            RP1C,   8, 
            RPNC,   8, 
            Offset (0xF3C), 
            TRAT,   8, 
            Offset (0xF40), 
            PTD1,   15, 
            Offset (0xF42), 
            TRA1,   8, 
            Offset (0xF44), 
            PMX1,   15, 
            Offset (0xF46), 
            PMN1,   15, 
            Offset (0xF48), 
            PTD2,   15, 
            Offset (0xF4A), 
            TRA2,   8, 
            Offset (0xF4C), 
            PMX2,   15, 
            Offset (0xF4E), 
            PMN2,   15, 
            Offset (0xF50), 
            CTCL,   2, 
                ,   29, 
            CLCK,   1, 
            MNTR,   8
        }

        Name (XPCC, Zero)
        Method (PPCC, 0, Serialized)
        {
            Return (NPCC) /* \_SB_.PCI0.B0D4.NPCC */
        }

        Name (NPCC, Package (0x03)
        {
            0x02, 
            Package (0x06)
            {
                Zero, 
                0x09C4, 
                0x2328, 
                0x5DC0, 
                0x6D60, 
                0x64
            }, 

            Package (0x06)
            {
                One, 
                0x1770, 
                0x3A98, 
                0x5DC0, 
                0x6D60, 
                0x64
            }
        })
        Method (CPNU, 2, Serialized)
        {
            Name (CNVT, Zero)
            Name (PPUU, Zero)
            Name (RMDR, Zero)
            If ((PWRU == Zero))
            {
                PPUU = One
            }
            Else
            {
                PPUU = (PWRU-- << 0x02)
            }

            Divide (Arg0, PPUU, RMDR, CNVT) /* \_SB_.PCI0.B0D4.CPNU.CNVT */
            If ((Arg1 == Zero))
            {
                Return (CNVT) /* \_SB_.PCI0.B0D4.CPNU.CNVT */
            }
            Else
            {
                CNVT *= 0x03E8
                RMDR *= 0x03E8
                RMDR /= PPUU
                CNVT += RMDR /* \_SB_.PCI0.B0D4.CPNU.RMDR */
                Return (CNVT) /* \_SB_.PCI0.B0D4.CPNU.CNVT */
            }
        }

        Method (CPL0, 0, NotSerialized)
        {
        }

        Method (CPL1, 0, NotSerialized)
        {
        }

        Method (CPL2, 0, NotSerialized)
        {
        }

        Name (LSTM, Zero)
        Name (_PPC, Zero)  // _PPC: Performance Present Capabilities
        Method (SPPC, 1, Serialized)
        {
            If (CondRefOf (\_SB.CPPC))
            {
                \_SB.CPPC = Arg0
            }

            Switch (ToInteger (\TCNT))
            {
                Case (0x14)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                    Notify (\_SB.PR08, 0x80) // Status Change
                    Notify (\_SB.PR09, 0x80) // Status Change
                    Notify (\_SB.PR10, 0x80) // Status Change
                    Notify (\_SB.PR11, 0x80) // Status Change
                    Notify (\_SB.PR12, 0x80) // Status Change
                    Notify (\_SB.PR13, 0x80) // Status Change
                    Notify (\_SB.PR14, 0x80) // Status Change
                    Notify (\_SB.PR15, 0x80) // Status Change
                    Notify (\_SB.PR16, 0x80) // Status Change
                    Notify (\_SB.PR17, 0x80) // Status Change
                    Notify (\_SB.PR18, 0x80) // Status Change
                    Notify (\_SB.PR19, 0x80) // Status Change
                }
                Case (0x13)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                    Notify (\_SB.PR08, 0x80) // Status Change
                    Notify (\_SB.PR09, 0x80) // Status Change
                    Notify (\_SB.PR10, 0x80) // Status Change
                    Notify (\_SB.PR11, 0x80) // Status Change
                    Notify (\_SB.PR12, 0x80) // Status Change
                    Notify (\_SB.PR13, 0x80) // Status Change
                    Notify (\_SB.PR14, 0x80) // Status Change
                    Notify (\_SB.PR15, 0x80) // Status Change
                    Notify (\_SB.PR16, 0x80) // Status Change
                    Notify (\_SB.PR17, 0x80) // Status Change
                    Notify (\_SB.PR18, 0x80) // Status Change
                }
                Case (0x12)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                    Notify (\_SB.PR08, 0x80) // Status Change
                    Notify (\_SB.PR09, 0x80) // Status Change
                    Notify (\_SB.PR10, 0x80) // Status Change
                    Notify (\_SB.PR11, 0x80) // Status Change
                    Notify (\_SB.PR12, 0x80) // Status Change
                    Notify (\_SB.PR13, 0x80) // Status Change
                    Notify (\_SB.PR14, 0x80) // Status Change
                    Notify (\_SB.PR15, 0x80) // Status Change
                    Notify (\_SB.PR16, 0x80) // Status Change
                    Notify (\_SB.PR17, 0x80) // Status Change
                }
                Case (0x11)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                    Notify (\_SB.PR08, 0x80) // Status Change
                    Notify (\_SB.PR09, 0x80) // Status Change
                    Notify (\_SB.PR10, 0x80) // Status Change
                    Notify (\_SB.PR11, 0x80) // Status Change
                    Notify (\_SB.PR12, 0x80) // Status Change
                    Notify (\_SB.PR13, 0x80) // Status Change
                    Notify (\_SB.PR14, 0x80) // Status Change
                    Notify (\_SB.PR15, 0x80) // Status Change
                    Notify (\_SB.PR16, 0x80) // Status Change
                }
                Case (0x10)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                    Notify (\_SB.PR08, 0x80) // Status Change
                    Notify (\_SB.PR09, 0x80) // Status Change
                    Notify (\_SB.PR10, 0x80) // Status Change
                    Notify (\_SB.PR11, 0x80) // Status Change
                    Notify (\_SB.PR12, 0x80) // Status Change
                    Notify (\_SB.PR13, 0x80) // Status Change
                    Notify (\_SB.PR14, 0x80) // Status Change
                    Notify (\_SB.PR15, 0x80) // Status Change
                }
                Case (0x0E)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                    Notify (\_SB.PR08, 0x80) // Status Change
                    Notify (\_SB.PR09, 0x80) // Status Change
                    Notify (\_SB.PR10, 0x80) // Status Change
                    Notify (\_SB.PR11, 0x80) // Status Change
                    Notify (\_SB.PR12, 0x80) // Status Change
                    Notify (\_SB.PR13, 0x80) // Status Change
                }
                Case (0x0C)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                    Notify (\_SB.PR08, 0x80) // Status Change
                    Notify (\_SB.PR09, 0x80) // Status Change
                    Notify (\_SB.PR10, 0x80) // Status Change
                    Notify (\_SB.PR11, 0x80) // Status Change
                }
                Case (0x0A)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                    Notify (\_SB.PR08, 0x80) // Status Change
                    Notify (\_SB.PR09, 0x80) // Status Change
                }
                Case (0x08)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                    Notify (\_SB.PR07, 0x80) // Status Change
                }
                Case (0x07)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                    Notify (\_SB.PR06, 0x80) // Status Change
                }
                Case (0x06)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                    Notify (\_SB.PR05, 0x80) // Status Change
                }
                Case (0x05)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                    Notify (\_SB.PR04, 0x80) // Status Change
                }
                Case (0x04)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                    Notify (\_SB.PR03, 0x80) // Status Change
                }
                Case (0x03)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                    Notify (\_SB.PR02, 0x80) // Status Change
                }
                Case (0x02)
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                    Notify (\_SB.PR01, 0x80) // Status Change
                }
                Default
                {
                    Notify (\_SB.PR00, 0x80) // Status Change
                }

            }
        }

        Name (TLPO, Package (0x06)
        {
            One, 
            One, 
            Zero, 
            One, 
            One, 
            0x02
        })
        Method (CLPO, 0, NotSerialized)
        {
            TLPO [One] = LPOE /* External reference */
            If (CondRefOf (\_SB.PR00._PSS))
            {
                If ((\_SB.OSCP & 0x0400))
                {
                    Local1 = SizeOf (\_SB.PR00.TPSS)
                }
                Else
                {
                    Local1 = SizeOf (\_SB.PR00.LPSS)
                }
            }
            Else
            {
                Local1 = Zero
            }

            If ((LPOP < Local1))
            {
                TLPO [0x02] = LPOP /* External reference */
            }
            Else
            {
                Local1--
                TLPO [0x02] = Local1
            }

            TLPO [0x03] = LPOS /* External reference */
            TLPO [0x04] = LPOW /* External reference */
            TLPO [0x05] = LPER /* External reference */
            Return (TLPO) /* \_SB_.PCI0.B0D4.TLPO */
        }

        Method (SPUR, 1, NotSerialized)
        {
            If ((Arg0 <= \TCNT))
            {
                If ((\_SB.PAGD._STA () == 0x0F))
                {
                    \_SB.PAGD._PUR [One] = Arg0
                    Notify (\_SB.PAGD, 0x80) // Status Change
                }
            }
        }

        Name (AEXL, Package (0x04)
        {
            "svchost.exe", 
            "dllhost.exe", 
            "smss.exe", 
            "WinSAT.exe"
        })
        Method (PCCC, 0, Serialized)
        {
            PCCX [Zero] = One
            Switch (ToInteger (CPNU (PTDP, Zero)))
            {
                Case (0x39)
                {
                    DerefOf (PCCX [One]) [Zero] = 0xA7F8
                    DerefOf (PCCX [One]) [One] = 0x00017318
                }
                Case (0x2F)
                {
                    DerefOf (PCCX [One]) [Zero] = 0x9858
                    DerefOf (PCCX [One]) [One] = 0x00014C08
                }
                Case (0x25)
                {
                    DerefOf (PCCX [One]) [Zero] = 0x7148
                    DerefOf (PCCX [One]) [One] = 0xD6D8
                }
                Case (0x19)
                {
                    DerefOf (PCCX [One]) [Zero] = 0x3E80
                    DerefOf (PCCX [One]) [One] = 0x7D00
                }
                Case (0x0F)
                {
                    DerefOf (PCCX [One]) [Zero] = 0x36B0
                    DerefOf (PCCX [One]) [One] = 0x7D00
                }
                Case (0x0B)
                {
                    DerefOf (PCCX [One]) [Zero] = 0x36B0
                    DerefOf (PCCX [One]) [One] = 0x61A8
                }
                Default
                {
                    DerefOf (PCCX [One]) [Zero] = 0xFF
                    DerefOf (PCCX [One]) [One] = 0xFF
                }

            }

            Return (PCCX) /* \_SB_.PCI0.B0D4.PCCX */
        }

        Name (PCCX, Package (0x02)
        {
            0x80000000, 
            Package (0x02)
            {
                0x80000000, 
                0x80000000
            }
        })
        Name (KEFF, Package (0x1E)
        {
            Package (0x02)
            {
                0x01BC, 
                Zero
            }, 

            Package (0x02)
            {
                0x01CF, 
                0x27
            }, 

            Package (0x02)
            {
                0x01E1, 
                0x4B
            }, 

            Package (0x02)
            {
                0x01F3, 
                0x6C
            }, 

            Package (0x02)
            {
                0x0206, 
                0x8B
            }, 

            Package (0x02)
            {
                0x0218, 
                0xA8
            }, 

            Package (0x02)
            {
                0x022A, 
                0xC3
            }, 

            Package (0x02)
            {
                0x023D, 
                0xDD
            }, 

            Package (0x02)
            {
                0x024F, 
                0xF4
            }, 

            Package (0x02)
            {
                0x0261, 
                0x010B
            }, 

            Package (0x02)
            {
                0x0274, 
                0x011F
            }, 

            Package (0x02)
            {
                0x032C, 
                0x01BD
            }, 

            Package (0x02)
            {
                0x03D7, 
                0x0227
            }, 

            Package (0x02)
            {
                0x048B, 
                0x026D
            }, 

            Package (0x02)
            {
                0x053E, 
                0x02A1
            }, 

            Package (0x02)
            {
                0x05F7, 
                0x02C6
            }, 

            Package (0x02)
            {
                0x06A8, 
                0x02E6
            }, 

            Package (0x02)
            {
                0x075D, 
                0x02FF
            }, 

            Package (0x02)
            {
                0x0818, 
                0x0311
            }, 

            Package (0x02)
            {
                0x08CF, 
                0x0322
            }, 

            Package (0x02)
            {
                0x179C, 
                0x0381
            }, 

            Package (0x02)
            {
                0x2DDC, 
                0x039C
            }, 

            Package (0x02)
            {
                0x44A8, 
                0x039E
            }, 

            Package (0x02)
            {
                0x5C35, 
                0x0397
            }, 

            Package (0x02)
            {
                0x747D, 
                0x038D
            }, 

            Package (0x02)
            {
                0x8D7F, 
                0x0382
            }, 

            Package (0x02)
            {
                0xA768, 
                0x0376
            }, 

            Package (0x02)
            {
                0xC23B, 
                0x0369
            }, 

            Package (0x02)
            {
                0xDE26, 
                0x035A
            }, 

            Package (0x02)
            {
                0xFB7C, 
                0x034A
            }
        })
        Name (CEUP, Package (0x06)
        {
            0x80000000, 
            0x80000000, 
            0x80000000, 
            0x80000000, 
            0x80000000, 
            0x80000000
        })
        Method (_TMP, 0, Serialized)  // _TMP: Temperature
        {
            If (\ECRD)
            {
                Local0 = \_SB.PCI0.LPCB.ECDV.KDRT (Zero)
                Return ((0x0AAC + (Local0 * 0x0A)))
            }
            Else
            {
                Return (0x0BB8)
            }
        }

        Method (_DTI, 1, NotSerialized)  // _DTI: Device Temperature Indication
        {
            LSTM = Arg0
            Notify (\_SB.PCI0.B0D4, 0x91) // Device-Specific
        }

        Method (_NTT, 0, NotSerialized)  // _NTT: Notification Temperature Threshold
        {
            Return (0x0ADE)
        }

        Name (PTYP, Zero)
        Method (_PSS, 0, NotSerialized)  // _PSS: Performance Supported States
        {
            If (CondRefOf (\_SB.PR00._PSS))
            {
                Return (\_SB.PR00._PSS ())
            }
            Else
            {
                Return (Package (0x02)
                {
                    Package (0x06)
                    {
                        Zero, 
                        Zero, 
                        Zero, 
                        Zero, 
                        Zero, 
                        Zero
                    }, 

                    Package (0x06)
                    {
                        Zero, 
                        Zero, 
                        Zero, 
                        Zero, 
                        Zero, 
                        Zero
                    }
                })
            }
        }

        Method (_TSS, 0, NotSerialized)  // _TSS: Throttling Supported States
        {
            If (CondRefOf (\_SB.PR00._TSS))
            {
                Return (\_SB.PR00._TSS ())
            }
            Else
            {
                Return (Package (0x01)
                {
                    Package (0x05)
                    {
                        One, 
                        Zero, 
                        Zero, 
                        Zero, 
                        Zero
                    }
                })
            }
        }

        Method (_TPC, 0, NotSerialized)  // _TPC: Throttling Present Capabilities
        {
            If (CondRefOf (\_SB.PR00._TPC))
            {
                Return (\_SB.PR00._TPC) /* External reference */
            }
            Else
            {
                Return (Zero)
            }
        }

        Method (_PTC, 0, NotSerialized)  // _PTC: Processor Throttling Control
        {
            If ((CondRefOf (\PC00) && (\PC00 != 0x80000000)))
            {
                If ((\PC00 & 0x04))
                {
                    Return (Package (0x02)
                    {
                        ResourceTemplate ()
                        {
                            Register (FFixedHW, 
                                0x00,               // Bit Width
                                0x00,               // Bit Offset
                                0x0000000000000000, // Address
                                ,)
                        }, 

                        ResourceTemplate ()
                        {
                            Register (FFixedHW, 
                                0x00,               // Bit Width
                                0x00,               // Bit Offset
                                0x0000000000000000, // Address
                                ,)
                        }
                    })
                }
                Else
                {
                    Return (Package (0x02)
                    {
                        ResourceTemplate ()
                        {
                            Register (SystemIO, 
                                0x05,               // Bit Width
                                0x00,               // Bit Offset
                                0x0000000000001810, // Address
                                ,)
                        }, 

                        ResourceTemplate ()
                        {
                            Register (SystemIO, 
                                0x05,               // Bit Width
                                0x00,               // Bit Offset
                                0x0000000000001810, // Address
                                ,)
                        }
                    })
                }
            }
            Else
            {
                Return (Package (0x02)
                {
                    ResourceTemplate ()
                    {
                        Register (FFixedHW, 
                            0x00,               // Bit Width
                            0x00,               // Bit Offset
                            0x0000000000000000, // Address
                            ,)
                    }, 

                    ResourceTemplate ()
                    {
                        Register (FFixedHW, 
                            0x00,               // Bit Width
                            0x00,               // Bit Offset
                            0x0000000000000000, // Address
                            ,)
                    }
                })
            }
        }

        Method (_TSD, 0, NotSerialized)  // _TSD: Throttling State Dependencies
        {
            If (CondRefOf (\_SB.PR00._TSD))
            {
                Return (\_SB.PR00._TSD ())
            }
            Else
            {
                Return (Package (0x01)
                {
                    Package (0x05)
                    {
                        0x05, 
                        Zero, 
                        Zero, 
                        0xFC, 
                        Zero
                    }
                })
            }
        }

        Method (_TDL, 0, NotSerialized)  // _TDL: T-State Depth Limit
        {
            If ((CondRefOf (\_SB.PR00._TSS) && CondRefOf (\_SB.CFGD)))
            {
                If ((\_SB.CFGD & 0x2000))
                {
                    Return ((SizeOf (\_SB.PR00.TSMF) - One))
                }
                Else
                {
                    Return ((SizeOf (\_SB.PR00.TSMC) - One))
                }
            }
            Else
            {
                Return (Zero)
            }
        }

        Method (_PDL, 0, NotSerialized)  // _PDL: P-state Depth Limit
        {
            If (CondRefOf (\_SB.PR00._PSS))
            {
                If ((\_SB.OSCP & 0x0400))
                {
                    Return ((SizeOf (\_SB.PR00.TPSS) - One))
                }
                Else
                {
                    Return ((SizeOf (\_SB.PR00.LPSS) - One))
                }
            }
            Else
            {
                Return (Zero)
            }
        }

        Method (_TSP, 0, Serialized)  // _TSP: Thermal Sampling Period
        {
            Return (\CPUS) /* External reference */
        }

        Method (_AC0, 0, Serialized)  // _ACx: Active Cooling, x=0-9
        {
            If ((\ATMC == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Local1 = \_SB.IETM.CTOK (\ATMC)
            If ((LSTM >= Local1))
            {
                Return ((Local1 - 0x14))
            }
            Else
            {
                Return (Local1)
            }
        }

        Method (_AC1, 0, Serialized)  // _ACx: Active Cooling, x=0-9
        {
            If ((\ATMC == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Local0 = \_SB.IETM.CTOK (\ATMC)
            Local0 -= 0x32
            If ((LSTM >= Local0))
            {
                Return ((Local0 - 0x14))
            }
            Else
            {
                Return (Local0)
            }
        }

        Method (_AC2, 0, Serialized)  // _ACx: Active Cooling, x=0-9
        {
            If ((\ATMC == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Local0 = \_SB.IETM.CTOK (\ATMC)
            Local0 -= 0x64
            If ((LSTM >= Local0))
            {
                Return ((Local0 - 0x14))
            }
            Else
            {
                Return (Local0)
            }
        }

        Method (_AC3, 0, Serialized)  // _ACx: Active Cooling, x=0-9
        {
            If ((\ATMC == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Local0 = \_SB.IETM.CTOK (\ATMC)
            Local0 -= 0x96
            If ((LSTM >= Local0))
            {
                Return ((Local0 - 0x14))
            }
            Else
            {
                Return (Local0)
            }
        }

        Method (_AC4, 0, Serialized)  // _ACx: Active Cooling, x=0-9
        {
            If ((\ATMC == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Local0 = \_SB.IETM.CTOK (\ATMC)
            Local0 -= 0xC8
            If ((LSTM >= Local0))
            {
                Return ((Local0 - 0x14))
            }
            Else
            {
                Return (Local0)
            }
        }

        Method (_PSV, 0, Serialized)  // _PSV: Passive Temperature
        {
            If ((\PTMC == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Return (\_SB.IETM.CTOK (\PTMC))
        }

        Method (_CRT, 0, Serialized)  // _CRT: Critical Temperature
        {
            If ((\SACT == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Return (\_SB.IETM.CTOK (\SACT))
        }

        Method (_CR3, 0, Serialized)  // _CR3: Warm/Standby Temperature
        {
            If ((\SAC3 == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Return (\_SB.IETM.CTOK (\SAC3))
        }

        Method (_HOT, 0, Serialized)  // _HOT: Hot Temperature
        {
            If ((\SAHT == Zero))
            {
                Return (0xFFFFFFFF)
            }

            Return (\_SB.IETM.CTOK (\SAHT))
        }
    }

    Scope (\_SB.IETM)
    {
        Name (CTSP, Package (0x01)
        {
            ToUUID ("e145970a-e4c1-4d73-900e-c9c5a69dd067") /* Unknown UUID */
        })
    }

    Scope (\_SB.PCI0.B0D4)
    {
        Method (TDPL, 0, Serialized)
        {
            Name (AAAA, Zero)
            Name (BBBB, Zero)
            Name (CCCC, Zero)
            Local0 = CTNL /* \_SB_.PCI0.B0D4.CTNL */
            If (((Local0 == One) || (Local0 == 0x02)))
            {
                Local0 = \_SB.CLVL /* External reference */
            }
            Else
            {
                Return (Package (0x01)
                {
                    Zero
                })
            }

            If ((CLCK == One))
            {
                Local0 = One
            }

            AAAA = CPNU (\_SB.PL10, One)
            BBBB = CPNU (\_SB.PL11, One)
            CCCC = CPNU (\_SB.PL12, One)
            Name (TMP1, Package (0x01)
            {
                Package (0x05)
                {
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000
                }
            })
            Name (TMP2, Package (0x02)
            {
                Package (0x05)
                {
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000
                }, 

                Package (0x05)
                {
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000
                }
            })
            Name (TMP3, Package (0x03)
            {
                Package (0x05)
                {
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000
                }, 

                Package (0x05)
                {
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000
                }, 

                Package (0x05)
                {
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000, 
                    0x80000000
                }
            })
            If ((Local0 == 0x03))
            {
                If ((AAAA > BBBB))
                {
                    If ((AAAA > CCCC))
                    {
                        If ((BBBB > CCCC))
                        {
                            Local3 = Zero
                            LEV0 = Zero
                            Local4 = One
                            LEV1 = One
                            Local5 = 0x02
                            LEV2 = 0x02
                        }
                        Else
                        {
                            Local3 = Zero
                            LEV0 = Zero
                            Local5 = One
                            LEV1 = 0x02
                            Local4 = 0x02
                            LEV2 = One
                        }
                    }
                    Else
                    {
                        Local5 = Zero
                        LEV0 = 0x02
                        Local3 = One
                        LEV1 = Zero
                        Local4 = 0x02
                        LEV2 = One
                    }
                }
                ElseIf ((BBBB > CCCC))
                {
                    If ((AAAA > CCCC))
                    {
                        Local4 = Zero
                        LEV0 = One
                        Local3 = One
                        LEV1 = Zero
                        Local5 = 0x02
                        LEV2 = 0x02
                    }
                    Else
                    {
                        Local4 = Zero
                        LEV0 = One
                        Local5 = One
                        LEV1 = 0x02
                        Local3 = 0x02
                        LEV2 = Zero
                    }
                }
                Else
                {
                    Local5 = Zero
                    LEV0 = 0x02
                    Local4 = One
                    LEV1 = One
                    Local3 = 0x02
                    LEV2 = Zero
                }

                Local1 = (\_SB.TAR0 + One)
                Local2 = (Local1 * 0x64)
                DerefOf (TMP3 [Local3]) [Zero] = AAAA /* \_SB_.PCI0.B0D4.TDPL.AAAA */
                DerefOf (TMP3 [Local3]) [One] = Local2
                DerefOf (TMP3 [Local3]) [0x02] = \_SB.CTC0 /* External reference */
                DerefOf (TMP3 [Local3]) [0x03] = Local1
                DerefOf (TMP3 [Local3]) [0x04] = Zero
                Local1 = (\_SB.TAR1 + One)
                Local2 = (Local1 * 0x64)
                DerefOf (TMP3 [Local4]) [Zero] = BBBB /* \_SB_.PCI0.B0D4.TDPL.BBBB */
                DerefOf (TMP3 [Local4]) [One] = Local2
                DerefOf (TMP3 [Local4]) [0x02] = \_SB.CTC1 /* External reference */
                DerefOf (TMP3 [Local4]) [0x03] = Local1
                DerefOf (TMP3 [Local4]) [0x04] = Zero
                Local1 = (\_SB.TAR2 + One)
                Local2 = (Local1 * 0x64)
                DerefOf (TMP3 [Local5]) [Zero] = CCCC /* \_SB_.PCI0.B0D4.TDPL.CCCC */
                DerefOf (TMP3 [Local5]) [One] = Local2
                DerefOf (TMP3 [Local5]) [0x02] = \_SB.CTC2 /* External reference */
                DerefOf (TMP3 [Local5]) [0x03] = Local1
                DerefOf (TMP3 [Local5]) [0x04] = Zero
                Return (TMP3) /* \_SB_.PCI0.B0D4.TDPL.TMP3 */
            }

            If ((Local0 == 0x02))
            {
                If ((AAAA > BBBB))
                {
                    Local3 = Zero
                    Local4 = One
                    LEV0 = Zero
                    LEV1 = One
                    LEV2 = Zero
                }
                Else
                {
                    Local4 = Zero
                    Local3 = One
                    LEV0 = One
                    LEV1 = Zero
                    LEV2 = Zero
                }

                Local1 = (\_SB.TAR0 + One)
                Local2 = (Local1 * 0x64)
                DerefOf (TMP2 [Local3]) [Zero] = AAAA /* \_SB_.PCI0.B0D4.TDPL.AAAA */
                DerefOf (TMP2 [Local3]) [One] = Local2
                DerefOf (TMP2 [Local3]) [0x02] = \_SB.CTC0 /* External reference */
                DerefOf (TMP2 [Local3]) [0x03] = Local1
                DerefOf (TMP2 [Local3]) [0x04] = Zero
                Local1 = (\_SB.TAR1 + One)
                Local2 = (Local1 * 0x64)
                DerefOf (TMP2 [Local4]) [Zero] = BBBB /* \_SB_.PCI0.B0D4.TDPL.BBBB */
                DerefOf (TMP2 [Local4]) [One] = Local2
                DerefOf (TMP2 [Local4]) [0x02] = \_SB.CTC1 /* External reference */
                DerefOf (TMP2 [Local4]) [0x03] = Local1
                DerefOf (TMP2 [Local4]) [0x04] = Zero
                Return (TMP2) /* \_SB_.PCI0.B0D4.TDPL.TMP2 */
            }

            If ((Local0 == One))
            {
                Switch (ToInteger (\_SB.CBMI))
                {
                    Case (Zero)
                    {
                        Local1 = (\_SB.TAR0 + One)
                        Local2 = (Local1 * 0x64)
                        DerefOf (TMP1 [Zero]) [Zero] = AAAA /* \_SB_.PCI0.B0D4.TDPL.AAAA */
                        DerefOf (TMP1 [Zero]) [One] = Local2
                        DerefOf (TMP1 [Zero]) [0x02] = \_SB.CTC0 /* External reference */
                        DerefOf (TMP1 [Zero]) [0x03] = Local1
                        DerefOf (TMP1 [Zero]) [0x04] = Zero
                        LEV0 = Zero
                        LEV1 = Zero
                        LEV2 = Zero
                    }
                    Case (One)
                    {
                        Local1 = (\_SB.TAR1 + One)
                        Local2 = (Local1 * 0x64)
                        DerefOf (TMP1 [Zero]) [Zero] = BBBB /* \_SB_.PCI0.B0D4.TDPL.BBBB */
                        DerefOf (TMP1 [Zero]) [One] = Local2
                        DerefOf (TMP1 [Zero]) [0x02] = \_SB.CTC1 /* External reference */
                        DerefOf (TMP1 [Zero]) [0x03] = Local1
                        DerefOf (TMP1 [Zero]) [0x04] = Zero
                        LEV0 = One
                        LEV1 = One
                        LEV2 = One
                    }
                    Case (0x02)
                    {
                        Local1 = (\_SB.TAR2 + One)
                        Local2 = (Local1 * 0x64)
                        DerefOf (TMP1 [Zero]) [Zero] = CCCC /* \_SB_.PCI0.B0D4.TDPL.CCCC */
                        DerefOf (TMP1 [Zero]) [One] = Local2
                        DerefOf (TMP1 [Zero]) [0x02] = \_SB.CTC2 /* External reference */
                        DerefOf (TMP1 [Zero]) [0x03] = Local1
                        DerefOf (TMP1 [Zero]) [0x04] = Zero
                        LEV0 = 0x02
                        LEV1 = 0x02
                        LEV2 = 0x02
                    }

                }

                Return (TMP1) /* \_SB_.PCI0.B0D4.TDPL.TMP1 */
            }

            Return (Zero)
        }

        Name (MAXT, Zero)
        Method (TDPC, 0, NotSerialized)
        {
            Return (MAXT) /* \_SB_.PCI0.B0D4.MAXT */
        }

        Name (LEV0, Zero)
        Name (LEV1, Zero)
        Name (LEV2, Zero)
        Method (STDP, 1, Serialized)
        {
            If ((Arg0 >= \_SB.CLVL))
            {
                Return (Zero)
            }

            Switch (ToInteger (Arg0))
            {
                Case (Zero)
                {
                    Local0 = LEV0 /* \_SB_.PCI0.B0D4.LEV0 */
                }
                Case (One)
                {
                    Local0 = LEV1 /* \_SB_.PCI0.B0D4.LEV1 */
                }
                Case (0x02)
                {
                    Local0 = LEV2 /* \_SB_.PCI0.B0D4.LEV2 */
                }

            }

            Switch (ToInteger (Local0))
            {
                Case (Zero)
                {
                    CPL0 ()
                }
                Case (One)
                {
                    CPL1 ()
                }
                Case (0x02)
                {
                    CPL2 ()
                }

            }

            Notify (\_SB.PCI0.B0D4, 0x83) // Device-Specific Change
        }
    }

    Scope (\_SB.PCI0.LPCB.ECDV)
    {
        Device (TMEM)
        {
            Name (_HID, EisaId ("INT3403") /* DPTF Temperature Sensor */)  // _HID: Hardware ID
            Name (_UID, "TMEM")  // _UID: Unique ID
            Name (_STR, Unicode ("Memory Participant"))  // _STR: Description String
            Name (PTYP, 0x03)
            Name (CTYP, Zero)
            Name (PFLG, Zero)
            Method (_STA, 0, NotSerialized)  // _STA: Status
            {
                If ((\S2DE == One))
                {
                    Return (0x0F)
                }
                Else
                {
                    Return (Zero)
                }
            }

            Method (_TMP, 0, Serialized)  // _TMP: Temperature
            {
                If (\ECRD)
                {
                    Local0 = \_SB.PCI0.LPCB.ECDV.KDRT (0x02)
                    Return ((0x0AAC + (Local0 * 0x0A)))
                }
                Else
                {
                    Return (0x0BB8)
                }
            }

            Name (PATC, 0x02)
            Method (PAT0, 1, Serialized)
            {
                If (\ECRD)
                {
                    Local0 = Acquire (\_SB.PCI0.LPCB.ECDV.PATM, 0x0064)
                    If ((Local0 == Zero))
                    {
                        Local1 = \_SB.IETM.KTOC (Arg0)
                        \_SB.PCI0.LPCB.ECDV.DSHY (0x02, 0x02)
                        \_SB.PCI0.LPCB.ECDV.DSTL (0x02, Local1)
                        Release (\_SB.PCI0.LPCB.ECDV.PATM)
                    }
                }
            }

            Method (PAT1, 1, Serialized)
            {
                If (\ECRD)
                {
                    Local0 = Acquire (\_SB.PCI0.LPCB.ECDV.PATM, 0x0064)
                    If ((Local0 == Zero))
                    {
                        Local1 = \_SB.IETM.KTOC (Arg0)
                        \_SB.PCI0.LPCB.ECDV.DSHY (0x02, 0x02)
                        \_SB.PCI0.LPCB.ECDV.DSTH (0x02, Local1)
                        Release (\_SB.PCI0.LPCB.ECDV.PATM)
                    }
                }
            }

            Name (GTSH, 0x28)
            Name (LSTM, Zero)
            Method (_DTI, 1, NotSerialized)  // _DTI: Device Temperature Indication
            {
                LSTM = Arg0
                Notify (\_SB.PCI0.LPCB.ECDV.TMEM, 0x91) // Device-Specific
            }

            Method (_NTT, 0, NotSerialized)  // _NTT: Notification Temperature Threshold
            {
                Return (0x0ADE)
            }

            Method (_TSP, 0, Serialized)  // _TSP: Thermal Sampling Period
            {
                Return (\SSP2) /* External reference */
            }

            Method (_AC0, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S2PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Local1 = \_SB.IETM.CTOK (\S2PT)
                }
                Else
                {
                    If ((\S2AT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Local1 = \_SB.IETM.CTOK (\S2AT)
                }

                If ((LSTM >= Local1))
                {
                    Return ((Local1 - 0x14))
                }
                Else
                {
                    Return (Local1)
                }
            }

            Method (_AC1, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S2PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }
                }
                ElseIf ((\S2AT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return ((_AC0 () - 0x64))
            }

            Method (_AC2, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S2PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }
                }
                ElseIf ((\S2AT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return ((_AC1 () - 0x64))
            }

            Method (_PSV, 0, Serialized)  // _PSV: Passive Temperature
            {
                If (CTYP)
                {
                    If ((\S2AT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Return (\_SB.IETM.CTOK (\S2AT))
                }
                Else
                {
                    If ((\S2PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Return (\_SB.IETM.CTOK (\S2PT))
                }
            }

            Method (_CRT, 0, Serialized)  // _CRT: Critical Temperature
            {
                If ((\S2CT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S2CT))
            }

            Method (_CR3, 0, Serialized)  // _CR3: Warm/Standby Temperature
            {
                If ((\S2S3 == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S2S3))
            }

            Method (_HOT, 0, Serialized)  // _HOT: Hot Temperature
            {
                If ((\S2HT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S2HT))
            }
        }
    }

    Scope (\_SB.PCI0.LPCB.ECDV)
    {
        Device (TSKN)
        {
            Name (_HID, EisaId ("INT3403") /* DPTF Temperature Sensor */)  // _HID: Hardware ID
            Name (_UID, "Skin")  // _UID: Unique ID
            Name (_STR, Unicode ("Skin Temperature Sensor(HT1)"))  // _STR: Description String
            Name (PTYP, 0x03)
            Name (CTYP, Zero)
            Name (PFLG, Zero)
            Method (_STA, 0, NotSerialized)  // _STA: Status
            {
                If ((\S3DE == One))
                {
                    Return (0x0F)
                }
                Else
                {
                    Return (Zero)
                }
            }

            Method (_TMP, 0, Serialized)  // _TMP: Temperature
            {
                If (\ECRD)
                {
                    Local0 = \_SB.PCI0.LPCB.ECDV.KDRT (One)
                    Return ((0x0AAC + (Local0 * 0x0A)))
                }
                Else
                {
                    Return (0x0BB8)
                }
            }

            Name (PATC, 0x02)
            Method (PAT0, 1, Serialized)
            {
                If (\ECRD)
                {
                    Local0 = Acquire (\_SB.PCI0.LPCB.ECDV.PATM, 0x0064)
                    If ((Local0 == Zero))
                    {
                        Local1 = \_SB.IETM.KTOC (Arg0)
                        \_SB.PCI0.LPCB.ECDV.DSHY (0x03, 0x02)
                        \_SB.PCI0.LPCB.ECDV.DSTL (0x03, Local1)
                        Release (\_SB.PCI0.LPCB.ECDV.PATM)
                    }
                }
            }

            Method (PAT1, 1, Serialized)
            {
                If (\ECRD)
                {
                    Local0 = Acquire (\_SB.PCI0.LPCB.ECDV.PATM, 0x0064)
                    If ((Local0 == Zero))
                    {
                        Local1 = \_SB.IETM.KTOC (Arg0)
                        \_SB.PCI0.LPCB.ECDV.DSHY (0x03, 0x02)
                        \_SB.PCI0.LPCB.ECDV.DSTH (0x03, Local1)
                        Release (\_SB.PCI0.LPCB.ECDV.PATM)
                    }
                }
            }

            Name (GTSH, 0x28)
            Name (LSTM, Zero)
            Method (_DTI, 1, NotSerialized)  // _DTI: Device Temperature Indication
            {
                LSTM = Arg0
                Notify (\_SB.PCI0.LPCB.ECDV.TSKN, 0x91) // Device-Specific
            }

            Method (_NTT, 0, NotSerialized)  // _NTT: Notification Temperature Threshold
            {
                Return (0x0ADE)
            }

            Method (_TSP, 0, Serialized)  // _TSP: Thermal Sampling Period
            {
                Return (\SSP3) /* External reference */
            }

            Method (_AC3, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S3PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Local1 = \_SB.IETM.CTOK (\S3PT)
                }
                Else
                {
                    If ((\S3AT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Local1 = \_SB.IETM.CTOK (\S3AT)
                }

                If ((LSTM >= Local1))
                {
                    Return ((Local1 - 0x14))
                }
                Else
                {
                    Return (Local1)
                }
            }

            Method (_AC4, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S3PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }
                }
                ElseIf ((\S3AT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return ((_AC3 () - 0x64))
            }

            Method (_AC5, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S3PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }
                }
                ElseIf ((\S3AT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return ((_AC4 () - 0x64))
            }

            Method (_PSV, 0, Serialized)  // _PSV: Passive Temperature
            {
                If (CTYP)
                {
                    If ((\S3AT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Return (\_SB.IETM.CTOK (\S3AT))
                }
                Else
                {
                    If ((\S3PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Return (\_SB.IETM.CTOK (\S3PT))
                }
            }

            Method (_CRT, 0, Serialized)  // _CRT: Critical Temperature
            {
                If ((\S3CT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S3CT))
            }

            Method (_CR3, 0, Serialized)  // _CR3: Warm/Standby Temperature
            {
                If ((\S3S3 == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S3S3))
            }

            Method (_HOT, 0, Serialized)  // _HOT: Hot Temperature
            {
                If ((\S3HT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S3HT))
            }
        }
    }

    Scope (\_SB.PCI0.LPCB.ECDV)
    {
        Device (NGFF)
        {
            Name (_HID, EisaId ("INT3403") /* DPTF Temperature Sensor */)  // _HID: Hardware ID
            Name (_UID, "NGFF")  // _UID: Unique ID
            Name (_STR, Unicode ("NGFF Temperature Sensor (HT3)"))  // _STR: Description String
            Name (PTYP, 0x03)
            Name (CTYP, Zero)
            Name (PFLG, Zero)
            Method (_STA, 0, NotSerialized)  // _STA: Status
            {
                If ((\S4DE == One))
                {
                    Return (0x0F)
                }
                Else
                {
                    Return (Zero)
                }
            }

            Method (_TMP, 0, Serialized)  // _TMP: Temperature
            {
                If (\ECRD)
                {
                    Local0 = \_SB.PCI0.LPCB.ECDV.KDRT (0x03)
                    Return ((0x0AAC + (Local0 * 0x0A)))
                }
                Else
                {
                    Return (0x0BB8)
                }
            }

            Name (PATC, 0x02)
            Method (PAT0, 1, Serialized)
            {
                If (\ECRD)
                {
                    Local0 = Acquire (\_SB.PCI0.LPCB.ECDV.PATM, 0x0064)
                    If ((Local0 == Zero))
                    {
                        Local1 = \_SB.IETM.KTOC (Arg0)
                        \_SB.PCI0.LPCB.ECDV.DSHY (0x04, 0x02)
                        \_SB.PCI0.LPCB.ECDV.DSTL (0x04, Local1)
                        Release (\_SB.PCI0.LPCB.ECDV.PATM)
                    }
                }
            }

            Method (PAT1, 1, Serialized)
            {
                If (\ECRD)
                {
                    Local0 = Acquire (\_SB.PCI0.LPCB.ECDV.PATM, 0x0064)
                    If ((Local0 == Zero))
                    {
                        Local1 = \_SB.IETM.KTOC (Arg0)
                        \_SB.PCI0.LPCB.ECDV.DSHY (0x04, 0x02)
                        \_SB.PCI0.LPCB.ECDV.DSTH (0x04, Local1)
                        Release (\_SB.PCI0.LPCB.ECDV.PATM)
                    }
                }
            }

            Name (GTSH, 0x28)
            Name (LSTM, Zero)
            Method (_DTI, 1, NotSerialized)  // _DTI: Device Temperature Indication
            {
                LSTM = Arg0
                Notify (\_SB.PCI0.LPCB.ECDV.NGFF, 0x91) // Device-Specific
            }

            Method (_NTT, 0, NotSerialized)  // _NTT: Notification Temperature Threshold
            {
                Return (0x0ADE)
            }

            Method (_TSP, 0, Serialized)  // _TSP: Thermal Sampling Period
            {
                Return (\SSP4) /* External reference */
            }

            Method (_AC0, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S4PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Local1 = \_SB.IETM.CTOK (\S4PT)
                }
                Else
                {
                    If ((\S4AT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Local1 = \_SB.IETM.CTOK (\S4AT)
                }

                If ((LSTM >= Local1))
                {
                    Return ((Local1 - 0x14))
                }
                Else
                {
                    Return (Local1)
                }
            }

            Method (_AC1, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S2PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }
                }
                ElseIf ((\S2AT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return ((_AC0 () - 0x64))
            }

            Method (_AC2, 0, Serialized)  // _ACx: Active Cooling, x=0-9
            {
                If (CTYP)
                {
                    If ((\S2PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }
                }
                ElseIf ((\S2AT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return ((_AC1 () - 0x64))
            }

            Method (_PSV, 0, Serialized)  // _PSV: Passive Temperature
            {
                If (CTYP)
                {
                    If ((\S4AT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Return (\_SB.IETM.CTOK (\S4AT))
                }
                Else
                {
                    If ((\S4PT == Zero))
                    {
                        Return (0xFFFFFFFF)
                    }

                    Return (\_SB.IETM.CTOK (\S4PT))
                }
            }

            Method (_CRT, 0, Serialized)  // _CRT: Critical Temperature
            {
                If ((\S4CT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S4CT))
            }

            Method (_CR3, 0, Serialized)  // _CR3: Warm/Standby Temperature
            {
                If ((\S4S3 == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S4S3))
            }

            Method (_HOT, 0, Serialized)  // _HOT: Hot Temperature
            {
                If ((\S4HT == Zero))
                {
                    Return (0xFFFFFFFF)
                }

                Return (\_SB.IETM.CTOK (\S4HT))
            }
        }
    }

    Scope (\_SB.IETM)
    {
        Name (TRT0, Package (0x03)
        {
            Package (0x08)
            {
                \_SB.PCI0.B0D4, 
                \_SB.PCI0.LPCB.ECDV.TMEM, 
                0x28, 
                0x64, 
                Zero, 
                Zero, 
                Zero, 
                Zero
            }, 

            Package (0x08)
            {
                \_SB.PCI0.B0D4, 
                \_SB.PCI0.LPCB.ECDV.TSKN, 
                0x1E, 
                0x96, 
                Zero, 
                Zero, 
                Zero, 
                Zero
            }, 

            Package (0x08)
            {
                \_SB.PCI0.B0D4, 
                \_SB.PCI0.LPCB.ECDV.NGFF, 
                0x14, 
                0xC8, 
                Zero, 
                Zero, 
                Zero, 
                Zero
            }
        })
        Method (_TRT, 0, NotSerialized)  // _TRT: Thermal Relationship Table
        {
            Return (TRT0) /* \_SB_.IETM.TRT0 */
        }
    }

    Scope (\_SB.IETM)
    {
        Name (PTTL, 0x14)
        Name (PSVT, Package (0x01)
        {
            0x02
        })
    }

    Scope (\_SB.IETM)
    {
        Name (DP2P, Package (0x01)
        {
            ToUUID ("9e04115a-ae87-4d1c-9500-0f3e340bfe75") /* Unknown UUID */
        })
        Name (DPSP, Package (0x01)
        {
            ToUUID ("42a441d6-ae6a-462b-a84b-4a8ce79027d3") /* Unknown UUID */
        })
        Name (DASP, Package (0x01)
        {
            ToUUID ("3a95c389-e4b8-4629-a526-c52c88626bae") /* Unknown UUID */
        })
        Name (DA2P, Package (0x01)
        {
            ToUUID ("0e56fab6-bdfc-4e8c-8246-40ecfd4d74ea") /* Unknown UUID */
        })
        Name (DCSP, Package (0x01)
        {
            ToUUID ("97c68ae7-15fa-499c-b8c9-5da81d606e0a") /* Unknown UUID */
        })
        Name (RFIP, Package (0x01)
        {
            ToUUID ("c4ce1849-243a-49f3-b8d5-f97002f38e6a") /* Unknown UUID */
        })
        Name (DAPP, Package (0x01)
        {
            ToUUID ("63be270f-1c11-48fd-a6f7-3af253ff3e2d") /* Unknown UUID */
        })
        Name (DPID, Package (0x01)
        {
            ToUUID ("42496e14-bc1b-46e8-a798-ca915464426f") /* Unknown UUID */
        })
    }

    Name (DBD0, Package (0x01)
    {
        Buffer (0x071D)
        {
            /* 0000 */  0xE5, 0x1F, 0x94, 0x00, 0x00, 0x00, 0x00, 0x02,  // ........
            /* 0008 */  0x00, 0x00, 0x00, 0x40, 0x67, 0x64, 0x64, 0x76,  // ...@gddv
            /* 0010 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0018 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0020 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0028 */  0x00, 0x00, 0x00, 0x00, 0x4F, 0x45, 0x4D, 0x20,  // ....OEM 
            /* 0030 */  0x45, 0x78, 0x70, 0x6F, 0x72, 0x74, 0x65, 0x64,  // Exported
            /* 0038 */  0x20, 0x44, 0x61, 0x74, 0x61, 0x56, 0x61, 0x75,  //  DataVau
            /* 0040 */  0x6C, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // lt......
            /* 0048 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0050 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0058 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0060 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0068 */  0x00, 0x00, 0x00, 0x00, 0x4D, 0x78, 0x7B, 0xF7,  // ....Mx{.
            /* 0070 */  0xC9, 0xF1, 0x2C, 0x42, 0x03, 0xA0, 0x35, 0x9E,  // ..,B..5.
            /* 0078 */  0xD7, 0xDB, 0x67, 0x7F, 0x77, 0x4B, 0x1D, 0x1F,  // ..g.wK..
            /* 0080 */  0x30, 0x56, 0xEA, 0xA1, 0x50, 0x07, 0xC4, 0x8B,  // 0V..P...
            /* 0088 */  0x8E, 0xC8, 0xE9, 0x12, 0x89, 0x06, 0x00, 0x00,  // ........
            /* 0090 */  0x52, 0x45, 0x50, 0x4F, 0x5D, 0x00, 0x00, 0x00,  // REPO]...
            /* 0098 */  0x01, 0x99, 0x67, 0x00, 0x00, 0x00, 0x00, 0x00,  // ..g.....
            /* 00A0 */  0x00, 0x00, 0x72, 0x87, 0xCD, 0xFF, 0x6D, 0x24,  // ..r...m$
            /* 00A8 */  0x47, 0xDB, 0x3D, 0x24, 0x92, 0xB4, 0x16, 0x6F,  // G.=$...o
            /* 00B0 */  0x45, 0xD8, 0xC3, 0xF5, 0x66, 0x14, 0x9F, 0x22,  // E...f.."
            /* 00B8 */  0xD7, 0xF7, 0xDE, 0x67, 0x90, 0x9A, 0xA2, 0x0D,  // ...g....
            /* 00C0 */  0x39, 0x25, 0xAD, 0xC3, 0x1A, 0xAD, 0x52, 0x0B,  // 9%....R.
            /* 00C8 */  0x75, 0x38, 0xE1, 0xA4, 0x14, 0x44, 0x14, 0x88,  // u8...D..
            /* 00D0 */  0xCE, 0xDA, 0xAB, 0xCE, 0x48, 0x18, 0xFD, 0xB0,  // ....H...
            /* 00D8 */  0x8A, 0xAF, 0x60, 0x1B, 0xB2, 0x10, 0x33, 0xD7,  // ..`...3.
            /* 00E0 */  0xC4, 0xC8, 0x9F, 0x9A, 0x66, 0xE5, 0x98, 0x1C,  // ....f...
            /* 00E8 */  0x5E, 0xFF, 0xB8, 0x9C, 0x9E, 0x17, 0x24, 0xA9,  // ^.....$.
            /* 00F0 */  0x89, 0x96, 0x87, 0xF7, 0x84, 0x98, 0xE5, 0xD4,  // ........
            /* 00F8 */  0xF3, 0x5D, 0xC3, 0x58, 0x04, 0x63, 0xD1, 0x46,  // .].X.c.F
            /* 0100 */  0xE1, 0xF2, 0x67, 0x7E, 0x60, 0x0A, 0xAE, 0xC3,  // ..g~`...
            /* 0108 */  0x25, 0xFE, 0x73, 0x8A, 0x3E, 0x13, 0x44, 0x3D,  // %.s.>.D=
            /* 0110 */  0x33, 0x23, 0x47, 0x36, 0xF8, 0x97, 0x30, 0xFC,  // 3#G6..0.
            /* 0118 */  0x15, 0x50, 0x60, 0x14, 0x12, 0xD2, 0xB9, 0xAC,  // .P`.....
            /* 0120 */  0x3F, 0x3F, 0xC0, 0x37, 0x88, 0xB7, 0x64, 0x2A,  // ??.7..d*
            /* 0128 */  0xA9, 0x23, 0xE0, 0x6E, 0x7E, 0x27, 0x27, 0xD8,  // .#.n~''.
            /* 0130 */  0x83, 0x27, 0xCC, 0x80, 0x1F, 0x5F, 0x4C, 0x55,  // .'..._LU
            /* 0138 */  0x97, 0xF3, 0x5B, 0x9C, 0x88, 0x8B, 0xAE, 0x0D,  // ..[.....
            /* 0140 */  0x6B, 0x8B, 0xF1, 0xE4, 0x14, 0x93, 0x63, 0x0C,  // k.....c.
            /* 0148 */  0x72, 0x8E, 0x6B, 0xB6, 0x91, 0x88, 0x29, 0xD6,  // r.k...).
            /* 0150 */  0xB0, 0xF9, 0xA1, 0xF5, 0x11, 0x93, 0xB6, 0x9D,  // ........
            /* 0158 */  0x74, 0x72, 0x63, 0x34, 0x50, 0x12, 0x43, 0x17,  // trc4P.C.
            /* 0160 */  0x23, 0xE7, 0x43, 0xCA, 0xB3, 0xC3, 0x22, 0xD4,  // #.C...".
            /* 0168 */  0x78, 0x46, 0x8E, 0xE8, 0x67, 0xC6, 0x6A, 0x39,  // xF..g.j9
            /* 0170 */  0x31, 0x8D, 0x17, 0xB6, 0xF4, 0xFE, 0x19, 0xB5,  // 1.......
            /* 0178 */  0x9C, 0x24, 0xB9, 0xC4, 0x25, 0x84, 0x55, 0x5D,  // .$..%.U]
            /* 0180 */  0xDA, 0x2B, 0xB0, 0xF8, 0xD3, 0x35, 0xC7, 0xB2,  // .+...5..
            /* 0188 */  0x8C, 0x7E, 0x5C, 0x16, 0xD8, 0x4A, 0x64, 0x3C,  // .~\..Jd<
            /* 0190 */  0xDD, 0x05, 0xF5, 0x8D, 0x75, 0xF7, 0x2E, 0xBF,  // ....u...
            /* 0198 */  0x7B, 0xE2, 0x15, 0xBA, 0x99, 0x8A, 0x1D, 0x2B,  // {......+
            /* 01A0 */  0x09, 0xAA, 0xF9, 0x4F, 0xF6, 0x54, 0xAE, 0xCF,  // ...O.T..
            /* 01A8 */  0xBC, 0xF1, 0xC5, 0x82, 0xFF, 0x9B, 0xE6, 0xB9,  // ........
            /* 01B0 */  0xD6, 0xD9, 0x5F, 0xA3, 0xCE, 0x27, 0xB3, 0xA0,  // .._..'..
            /* 01B8 */  0xCD, 0xE2, 0x18, 0xD6, 0x6C, 0x6D, 0xF7, 0x35,  // ....lm.5
            /* 01C0 */  0x5B, 0xA2, 0x61, 0x61, 0x7E, 0x83, 0x1D, 0x87,  // [.aa~...
            /* 01C8 */  0x26, 0x5C, 0x68, 0x45, 0x8F, 0xD4, 0xEF, 0x7D,  // &\hE...}
            /* 01D0 */  0x77, 0x8E, 0x1C, 0xE6, 0x1F, 0xC9, 0x12, 0xA8,  // w.......
            /* 01D8 */  0x3E, 0x41, 0x0A, 0xCB, 0x1B, 0xF9, 0xE2, 0x56,  // >A.....V
            /* 01E0 */  0x13, 0x86, 0xC2, 0x58, 0xE2, 0x71, 0x26, 0xAC,  // ...X.q&.
            /* 01E8 */  0x53, 0xA7, 0x09, 0x6F, 0x13, 0x9F, 0x74, 0xBF,  // S..o..t.
            /* 01F0 */  0x11, 0x69, 0x05, 0xE7, 0xE4, 0x83, 0xCC, 0x5F,  // .i....._
            /* 01F8 */  0xBF, 0x8E, 0x7C, 0xB7, 0x66, 0xE7, 0xEA, 0x30,  // ..|.f..0
            /* 0200 */  0x36, 0x55, 0x28, 0x38, 0x4D, 0xD7, 0x67, 0xFD,  // 6U(8M.g.
            /* 0208 */  0x53, 0xE0, 0x2F, 0xE7, 0x2E, 0xC0, 0x66, 0x5B,  // S./...f[
            /* 0210 */  0xB9, 0x90, 0x6A, 0x27, 0x5B, 0x15, 0x37, 0xB4,  // ..j'[.7.
            /* 0218 */  0x36, 0x3E, 0xDF, 0x94, 0x52, 0x50, 0x7E, 0x21,  // 6>..RP~!
            /* 0220 */  0x45, 0x53, 0x3E, 0xC4, 0xA8, 0xFE, 0xDF, 0xB5,  // ES>.....
            /* 0228 */  0xA7, 0xBE, 0xF9, 0x90, 0xB2, 0x42, 0xCC, 0x6A,  // .....B.j
            /* 0230 */  0x7B, 0x4B, 0xC0, 0xD1, 0x1E, 0xB0, 0x53, 0x89,  // {K....S.
            /* 0238 */  0xDB, 0xEA, 0xF4, 0x36, 0x8C, 0xAC, 0x28, 0x52,  // ...6..(R
            /* 0240 */  0x89, 0x0C, 0x97, 0xFC, 0x9E, 0xE4, 0x90, 0xE2,  // ........
            /* 0248 */  0xFE, 0x70, 0x6E, 0x65, 0x20, 0x7E, 0xED, 0x5E,  // .pne ~.^
            /* 0250 */  0x47, 0xEC, 0xB0, 0x0C, 0x38, 0xAF, 0x07, 0x81,  // G...8...
            /* 0258 */  0x2D, 0x79, 0xB8, 0xEF, 0xA0, 0x09, 0x82, 0xEF,  // -y......
            /* 0260 */  0xB9, 0x3D, 0x22, 0x34, 0x83, 0xC9, 0xEB, 0xEC,  // .="4....
            /* 0268 */  0x1D, 0x55, 0xC1, 0xE1, 0x61, 0x4B, 0xF3, 0x68,  // .U..aK.h
            /* 0270 */  0x0F, 0xFF, 0x06, 0xAA, 0x09, 0xCF, 0xAA, 0x3E,  // .......>
            /* 0278 */  0xA4, 0x90, 0x88, 0x00, 0x68, 0xEE, 0x99, 0xA8,  // ....h...
            /* 0280 */  0x5D, 0x72, 0x80, 0x7E, 0x0C, 0xA2, 0xE7, 0x56,  // ]r.~...V
            /* 0288 */  0xA6, 0xD5, 0xD2, 0x72, 0x6F, 0xE6, 0xA2, 0x65,  // ...ro..e
            /* 0290 */  0x4D, 0x11, 0xC2, 0xC1, 0xCE, 0x1A, 0xCA, 0xE4,  // M.......
            /* 0298 */  0x61, 0x23, 0x45, 0x65, 0x36, 0x6B, 0x99, 0xA2,  // a#Ee6k..
            /* 02A0 */  0x48, 0x4A, 0xE5, 0x61, 0x15, 0xDA, 0xF8, 0x85,  // HJ.a....
            /* 02A8 */  0x54, 0x57, 0x37, 0x23, 0x87, 0x59, 0x91, 0x99,  // TW7#.Y..
            /* 02B0 */  0x48, 0xED, 0x9C, 0xC0, 0xE2, 0xD9, 0x17, 0xDA,  // H.......
            /* 02B8 */  0xA8, 0xF7, 0x20, 0xA0, 0x2A, 0x53, 0x71, 0x17,  // .. .*Sq.
            /* 02C0 */  0x25, 0x82, 0x35, 0x13, 0x64, 0x17, 0x19, 0xB8,  // %.5.d...
            /* 02C8 */  0x48, 0xDB, 0xEB, 0x4C, 0x1D, 0x6E, 0xF9, 0x20,  // H..L.n. 
            /* 02D0 */  0xD9, 0xED, 0x19, 0x38, 0xD7, 0x21, 0x4F, 0x28,  // ...8.!O(
            /* 02D8 */  0x79, 0xBD, 0x96, 0xB3, 0x90, 0x88, 0x65, 0xED,  // y.....e.
            /* 02E0 */  0xCE, 0x08, 0x95, 0xF1, 0xFE, 0x54, 0xF4, 0x9E,  // .....T..
            /* 02E8 */  0xA5, 0x77, 0x1B, 0x8C, 0x2E, 0x00, 0x1F, 0xDD,  // .w......
            /* 02F0 */  0x4F, 0x82, 0x6E, 0x69, 0x73, 0x1A, 0xE5, 0x7A,  // O.nis..z
            /* 02F8 */  0x7C, 0x68, 0x5E, 0x2F, 0x4E, 0x66, 0xCC, 0xED,  // |h^/Nf..
            /* 0300 */  0x62, 0x5E, 0x3B, 0xD7, 0x63, 0xED, 0x1B, 0xE1,  // b^;.c...
            /* 0308 */  0xE3, 0x7A, 0x25, 0xB8, 0x57, 0xBF, 0xB2, 0x50,  // .z%.W..P
            /* 0310 */  0x2B, 0x85, 0xA4, 0xD0, 0x78, 0x43, 0xB4, 0x61,  // +...xC.a
            /* 0318 */  0x5B, 0xD4, 0x8A, 0x1B, 0xB8, 0xB8, 0x90, 0x12,  // [.......
            /* 0320 */  0x23, 0x2C, 0xFD, 0xAC, 0xBE, 0xE1, 0x0D, 0x10,  // #,......
            /* 0328 */  0x76, 0x52, 0xA4, 0xC4, 0x97, 0x86, 0xCD, 0xF2,  // vR......
            /* 0330 */  0x59, 0x23, 0xF2, 0xE8, 0x40, 0xED, 0x80, 0x19,  // Y#..@...
            /* 0338 */  0xEB, 0x80, 0x37, 0x27, 0x43, 0x07, 0xB5, 0x86,  // ..7'C...
            /* 0340 */  0x29, 0x4D, 0x33, 0x68, 0x00, 0x4D, 0x4A, 0xC5,  // )M3h.MJ.
            /* 0348 */  0xA7, 0xE6, 0x8C, 0x1B, 0xF8, 0x6D, 0x3E, 0x9C,  // .....m>.
            /* 0350 */  0xCB, 0xBB, 0x5D, 0x8A, 0x03, 0x57, 0xE3, 0x71,  // ..]..W.q
            /* 0358 */  0xB8, 0x00, 0xB5, 0x3B, 0x0C, 0xD7, 0x03, 0xF0,  // ...;....
            /* 0360 */  0x12, 0xBB, 0x5F, 0xDD, 0x05, 0x34, 0x77, 0x97,  // .._..4w.
            /* 0368 */  0x6F, 0x27, 0x38, 0x3E, 0x25, 0xBE, 0xE1, 0x53,  // o'8>%..S
            /* 0370 */  0x57, 0x1C, 0x9F, 0xB1, 0x3F, 0x3E, 0x1F, 0xB5,  // W...?>..
            /* 0378 */  0x41, 0xAA, 0x31, 0x77, 0x29, 0xD6, 0xB1, 0x94,  // A.1w)...
            /* 0380 */  0x54, 0x55, 0x72, 0xE9, 0x52, 0x83, 0x1D, 0xDE,  // TUr.R...
            /* 0388 */  0x26, 0xBD, 0x8B, 0xA7, 0x14, 0x98, 0xE2, 0x88,  // &.......
            /* 0390 */  0x78, 0xB4, 0xEE, 0x13, 0x8E, 0xE3, 0xD2, 0x4A,  // x......J
            /* 0398 */  0x6C, 0x4B, 0x6B, 0xA7, 0x0D, 0x13, 0x86, 0x92,  // lKk.....
            /* 03A0 */  0xBF, 0xAE, 0xD7, 0x0C, 0x1F, 0x2D, 0xB5, 0x6D,  // .....-.m
            /* 03A8 */  0x1A, 0x2B, 0x4F, 0x42, 0x72, 0x4F, 0x00, 0x91,  // .+OBrO..
            /* 03B0 */  0x8A, 0x76, 0xE2, 0xB9, 0x49, 0x2F, 0x2D, 0xBD,  // .v..I/-.
            /* 03B8 */  0x20, 0x23, 0xD2, 0x90, 0xEF, 0x2D, 0x98, 0x82,  //  #...-..
            /* 03C0 */  0x83, 0xD7, 0x56, 0x76, 0x16, 0x7C, 0x60, 0x32,  // ..Vv.|`2
            /* 03C8 */  0xB5, 0x94, 0xAF, 0x6E, 0x4D, 0xCF, 0x59, 0xB9,  // ...nM.Y.
            /* 03D0 */  0x27, 0x3A, 0x1F, 0xC3, 0xF4, 0xD7, 0xAC, 0x35,  // ':.....5
            /* 03D8 */  0xA4, 0x78, 0x3C, 0xA6, 0xFA, 0x8B, 0x29, 0x37,  // .x<...)7
            /* 03E0 */  0x80, 0xCE, 0x21, 0xFA, 0xE0, 0xEC, 0x80, 0x5E,  // ..!....^
            /* 03E8 */  0x69, 0xEE, 0xF6, 0xD5, 0x1C, 0x62, 0xFB, 0x68,  // i....b.h
            /* 03F0 */  0x2F, 0x74, 0x9D, 0x67, 0xB0, 0x7D, 0x8B, 0xF3,  // /t.g.}..
            /* 03F8 */  0x17, 0x78, 0x6F, 0xB6, 0x41, 0x19, 0x8D, 0xED,  // .xo.A...
            /* 0400 */  0x11, 0x16, 0x29, 0x5C, 0x7C, 0xC7, 0x4A, 0x08,  // ..)\|.J.
            /* 0408 */  0xDC, 0x97, 0x9A, 0x2E, 0x7B, 0xC0, 0x92, 0x61,  // ....{..a
            /* 0410 */  0x76, 0x8C, 0x1A, 0x79, 0x7D, 0xFE, 0x89, 0x54,  // v..y}..T
            /* 0418 */  0xF5, 0xDD, 0x73, 0x87, 0xDF, 0xBA, 0xA1, 0x27,  // ..s....'
            /* 0420 */  0xD6, 0x87, 0x40, 0x3A, 0xF4, 0x40, 0xC9, 0xF5,  // ..@:.@..
            /* 0428 */  0x77, 0x53, 0x57, 0x9F, 0xA0, 0xD5, 0x77, 0x78,  // wSW...wx
            /* 0430 */  0xD4, 0xA5, 0x23, 0x34, 0xD1, 0x33, 0xEC, 0x0D,  // ..#4.3..
            /* 0438 */  0x7B, 0x7D, 0x22, 0x29, 0x5C, 0x96, 0x52, 0x4C,  // {}")\.RL
            /* 0440 */  0xAA, 0xFF, 0x26, 0x0D, 0xEC, 0x35, 0x0B, 0x93,  // ..&..5..
            /* 0448 */  0x4A, 0xA8, 0x75, 0x38, 0xE1, 0x9E, 0x99, 0xA2,  // J.u8....
            /* 0450 */  0xA5, 0x8B, 0xA5, 0x86, 0x61, 0xC7, 0xCE, 0x15,  // ....a...
            /* 0458 */  0x3C, 0x70, 0xAC, 0xDA, 0x86, 0xFC, 0xA5, 0x84,  // <p......
            /* 0460 */  0x0F, 0x5B, 0x98, 0x07, 0x46, 0xD0, 0x1C, 0x7D,  // .[..F..}
            /* 0468 */  0x7B, 0xD3, 0xCF, 0xFC, 0xAC, 0xC7, 0xFD, 0x19,  // {.......
            /* 0470 */  0xB4, 0xCB, 0xD8, 0xF1, 0xD4, 0x55, 0x3A, 0x9B,  // .....U:.
            /* 0478 */  0xA4, 0xDF, 0xA0, 0x6A, 0x8E, 0x3C, 0xAD, 0xA3,  // ...j.<..
            /* 0480 */  0x54, 0xA5, 0x85, 0xC8, 0x8E, 0x6F, 0x81, 0x3C,  // T....o.<
            /* 0488 */  0xD7, 0x91, 0xE8, 0x7E, 0x81, 0x82, 0x98, 0xD1,  // ...~....
            /* 0490 */  0x37, 0xDF, 0xE2, 0x20, 0xB8, 0x8E, 0xB4, 0x68,  // 7.. ...h
            /* 0498 */  0x70, 0x18, 0x76, 0x00, 0xED, 0xED, 0x34, 0x26,  // p.v...4&
            /* 04A0 */  0xF2, 0x4F, 0x36, 0x46, 0xA3, 0xFD, 0x3D, 0x10,  // .O6F..=.
            /* 04A8 */  0x1F, 0x0E, 0x3F, 0x0A, 0x25, 0x42, 0x16, 0x3C,  // ..?.%B.<
            /* 04B0 */  0xB4, 0x7B, 0xFB, 0x61, 0xA6, 0x96, 0x5B, 0xF4,  // .{.a..[.
            /* 04B8 */  0x2B, 0x6A, 0x33, 0x3F, 0xC5, 0x9F, 0xE5, 0x49,  // +j3?...I
            /* 04C0 */  0x89, 0x27, 0x26, 0xA4, 0x4D, 0xF3, 0x19, 0x00,  // .'&.M...
            /* 04C8 */  0xDB, 0x99, 0x0D, 0xCF, 0xCE, 0x41, 0xFA, 0x6E,  // .....A.n
            /* 04D0 */  0x48, 0xA7, 0x2F, 0x7E, 0xC8, 0xBD, 0xCA, 0x4F,  // H./~...O
            /* 04D8 */  0x26, 0x39, 0x41, 0x56, 0xAE, 0x49, 0x30, 0x96,  // &9AV.I0.
            /* 04E0 */  0x5A, 0x39, 0x70, 0x9A, 0xF1, 0xD8, 0xFA, 0xEB,  // Z9p.....
            /* 04E8 */  0x48, 0xAD, 0xBA, 0xA3, 0xDF, 0x7B, 0x53, 0x09,  // H....{S.
            /* 04F0 */  0x16, 0xAC, 0x59, 0x54, 0x8A, 0x58, 0x28, 0x67,  // ..YT.X(g
            /* 04F8 */  0xA3, 0x53, 0xCA, 0xCD, 0x5D, 0x2D, 0x7D, 0x8A,  // .S..]-}.
            /* 0500 */  0x52, 0x06, 0xCC, 0x91, 0xAB, 0x49, 0x3B, 0x21,  // R....I;!
            /* 0508 */  0x28, 0x21, 0xF4, 0xB9, 0x86, 0x6D, 0x81, 0x5C,  // (!...m.\
            /* 0510 */  0x1B, 0x1D, 0x04, 0xC3, 0xC5, 0x05, 0xFF, 0x38,  // .......8
            /* 0518 */  0x78, 0x7B, 0xE6, 0x9B, 0x68, 0xD1, 0x90, 0x49,  // x{..h..I
            /* 0520 */  0x41, 0xA7, 0x74, 0xB4, 0x55, 0x64, 0x65, 0x77,  // A.t.Udew
            /* 0528 */  0x48, 0xDA, 0x3F, 0xB5, 0xAA, 0xF1, 0xC3, 0xAE,  // H.?.....
            /* 0530 */  0x43, 0x89, 0x4B, 0x81, 0x74, 0x1C, 0x63, 0x6A,  // C.K.t.cj
            /* 0538 */  0x03, 0xE0, 0x84, 0x29, 0x34, 0x5C, 0x9D, 0x83,  // ...)4\..
            /* 0540 */  0xB3, 0x47, 0xB6, 0x8B, 0x30, 0x35, 0xA9, 0xCD,  // .G..05..
            /* 0548 */  0x6C, 0xA9, 0xF7, 0x8E, 0xE6, 0x6C, 0xBA, 0x73,  // l....l.s
            /* 0550 */  0xFE, 0x20, 0x7C, 0xE6, 0x59, 0x7E, 0x8B, 0x27,  // . |.Y~.'
            /* 0558 */  0x61, 0xF6, 0xE0, 0x73, 0x50, 0xD3, 0x7D, 0x29,  // a..sP.})
            /* 0560 */  0x5D, 0xF4, 0xF9, 0xE8, 0x76, 0x94, 0x59, 0x1D,  // ]...v.Y.
            /* 0568 */  0xBC, 0x52, 0x6D, 0x14, 0x09, 0xB7, 0x7E, 0xEB,  // .Rm...~.
            /* 0570 */  0x25, 0x13, 0x5F, 0x3E, 0x0A, 0x78, 0x65, 0xEB,  // %._>.xe.
            /* 0578 */  0x2B, 0xF6, 0xF6, 0x50, 0xF4, 0xE5, 0xD2, 0x32,  // +..P...2
            /* 0580 */  0x7B, 0x51, 0x62, 0xF6, 0x9D, 0xC2, 0xD8, 0x15,  // {Qb.....
            /* 0588 */  0x77, 0x38, 0x9D, 0xF3, 0x65, 0xEE, 0x01, 0x11,  // w8..e...
            /* 0590 */  0xF7, 0xD3, 0x31, 0x6A, 0x90, 0xFD, 0x7A, 0x2A,  // ..1j..z*
            /* 0598 */  0x1F, 0xD0, 0x60, 0xC7, 0x6F, 0x2D, 0x47, 0x2E,  // ..`.o-G.
            /* 05A0 */  0x06, 0x88, 0x84, 0xF9, 0x4F, 0x81, 0x54, 0xC9,  // ....O.T.
            /* 05A8 */  0xFA, 0xD9, 0x7F, 0x0B, 0xC9, 0x98, 0x7B, 0xEE,  // ......{.
            /* 05B0 */  0x37, 0xB7, 0xC1, 0xB5, 0xFD, 0xB4, 0x5D, 0x1B,  // 7.....].
            /* 05B8 */  0x9B, 0xEE, 0xC1, 0x86, 0xC2, 0x60, 0x34, 0x2B,  // .....`4+
            /* 05C0 */  0x82, 0xF0, 0x51, 0xD7, 0x58, 0x39, 0x65, 0x3D,  // ..Q.X9e=
            /* 05C8 */  0xA3, 0x5D, 0xBE, 0x99, 0x01, 0x9F, 0x05, 0xA1,  // .]......
            /* 05D0 */  0xE4, 0x2F, 0xB1, 0x03, 0x84, 0xCF, 0x9C, 0x9D,  // ./......
            /* 05D8 */  0x0E, 0x4D, 0x1A, 0x09, 0xD3, 0x57, 0x5C, 0x55,  // .M...W\U
            /* 05E0 */  0x57, 0x1D, 0xB8, 0x72, 0x6A, 0xEF, 0x4E, 0xBC,  // W..rj.N.
            /* 05E8 */  0xD6, 0x41, 0x78, 0xA0, 0x5A, 0xC0, 0x7C, 0x35,  // .Ax.Z.|5
            /* 05F0 */  0x5E, 0x0E, 0x3E, 0x44, 0x1F, 0x5F, 0x26, 0x32,  // ^.>D._&2
            /* 05F8 */  0x59, 0x79, 0x95, 0xB7, 0x9F, 0x14, 0x3F, 0x68,  // Yy....?h
            /* 0600 */  0x66, 0x8B, 0x08, 0x39, 0x6F, 0xE1, 0x21, 0xE7,  // f..9o.!.
            /* 0608 */  0x77, 0x84, 0x7F, 0x1D, 0x92, 0x31, 0x39, 0xCE,  // w....19.
            /* 0610 */  0x6F, 0x76, 0xAC, 0x0E, 0x8A, 0x15, 0x3A, 0xE9,  // ov....:.
            /* 0618 */  0x56, 0xCE, 0x42, 0xE4, 0x9F, 0x6D, 0xCE, 0x4E,  // V.B..m.N
            /* 0620 */  0x08, 0xD6, 0xF6, 0xDE, 0xF2, 0x8B, 0x60, 0x41,  // ......`A
            /* 0628 */  0x55, 0xAC, 0xF9, 0xCE, 0xFA, 0x7B, 0xC6, 0x79,  // U....{.y
            /* 0630 */  0x0A, 0x0A, 0x87, 0xB4, 0x86, 0x6E, 0xB3, 0x42,  // .....n.B
            /* 0638 */  0x78, 0x4C, 0x33, 0xEB, 0x6F, 0xD0, 0x57, 0x7A,  // xL3.o.Wz
            /* 0640 */  0x1A, 0xE7, 0x73, 0xB9, 0xBD, 0x61, 0x62, 0x2B,  // ..s..ab+
            /* 0648 */  0x7B, 0xD4, 0xDA, 0xCE, 0x78, 0xBC, 0xE5, 0x9D,  // {...x...
            /* 0650 */  0xD6, 0x8A, 0x32, 0x48, 0xB5, 0x61, 0x6B, 0x65,  // ..2H.ake
            /* 0658 */  0x02, 0x29, 0xD1, 0x70, 0x16, 0x5F, 0xEC, 0x3C,  // .).p._.<
            /* 0660 */  0x4A, 0x8A, 0x84, 0xB0, 0x6E, 0x16, 0x15, 0xFE,  // J...n...
            /* 0668 */  0xF3, 0xE2, 0xE8, 0x30, 0x29, 0x3F, 0x1E, 0xDF,  // ...0)?..
            /* 0670 */  0xE4, 0x7D, 0xB4, 0x41, 0xFA, 0xB2, 0x77, 0x81,  // .}.A..w.
            /* 0678 */  0xB1, 0x86, 0x4F, 0x4F, 0x14, 0x9F, 0xE6, 0xD1,  // ..OO....
            /* 0680 */  0x62, 0xF1, 0x17, 0xB9, 0xBD, 0xBE, 0x3F, 0xA4,  // b.....?.
            /* 0688 */  0x6B, 0x49, 0xE5, 0x13, 0x90, 0x2D, 0x41, 0x07,  // kI...-A.
            /* 0690 */  0xC6, 0xFE, 0xDD, 0xF9, 0xCF, 0x04, 0x86, 0x8E,  // ........
            /* 0698 */  0x5B, 0x61, 0x96, 0xDC, 0xC2, 0xA6, 0x58, 0x58,  // [a....XX
            /* 06A0 */  0x83, 0xD3, 0x30, 0xA4, 0x63, 0x62, 0x3C, 0x22,  // ..0.cb<"
            /* 06A8 */  0xFB, 0x50, 0xF9, 0x2F, 0xB8, 0xA0, 0x79, 0xBB,  // .P./..y.
            /* 06B0 */  0x6D, 0xF8, 0x1A, 0x66, 0xB5, 0xF0, 0x3A, 0xFD,  // m..f..:.
            /* 06B8 */  0xA0, 0xB9, 0x36, 0x75, 0xFE, 0x69, 0x80, 0x79,  // ..6u.i.y
            /* 06C0 */  0x70, 0xC5, 0xC8, 0x69, 0xE5, 0xD8, 0x29, 0x1D,  // p..i..).
            /* 06C8 */  0xA9, 0xBB, 0xD1, 0xE4, 0xE5, 0xEB, 0xC4, 0x14,  // ........
            /* 06D0 */  0x80, 0x16, 0x9A, 0x01, 0xC8, 0x4F, 0x07, 0x0E,  // .....O..
            /* 06D8 */  0x40, 0xF7, 0x5C, 0x36, 0x8F, 0x54, 0x13, 0xFE,  // @.\6.T..
            /* 06E0 */  0x8F, 0x2D, 0x46, 0xC6, 0xB2, 0xFC, 0x65, 0x1C,  // .-F...e.
            /* 06E8 */  0x38, 0x8E, 0xE1, 0x0C, 0x67, 0xE9, 0x2E, 0xD5,  // 8...g...
            /* 06F0 */  0x74, 0x6D, 0x9A, 0x41, 0x66, 0x3A, 0x77, 0x18,  // tm.Af:w.
            /* 06F8 */  0x70, 0x02, 0x29, 0x4C, 0x57, 0xC1, 0xAA, 0x56,  // p.)LW..V
            /* 0700 */  0xA2, 0x63, 0xCF, 0x37, 0x78, 0xA6, 0x90, 0x42,  // .c.7x..B
            /* 0708 */  0x76, 0xDE, 0x70, 0xD5, 0x78, 0x72, 0xF5, 0xF8,  // v.p.xr..
            /* 0710 */  0x1D, 0x17, 0x45, 0xC0, 0xF4, 0x7A, 0x99, 0x9D,  // ..E..z..
            /* 0718 */  0x61, 0x13, 0xBF, 0x06, 0x00                     // a....
        }
    })
    Name (DBD1, Package (0x01)
    {
        Buffer (0x074C)
        {
            /* 0000 */  0xE5, 0x1F, 0x94, 0x00, 0x00, 0x00, 0x00, 0x02,  // ........
            /* 0008 */  0x00, 0x00, 0x00, 0x40, 0x67, 0x64, 0x64, 0x76,  // ...@gddv
            /* 0010 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0018 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0020 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0028 */  0x00, 0x00, 0x00, 0x00, 0x4F, 0x45, 0x4D, 0x20,  // ....OEM 
            /* 0030 */  0x45, 0x78, 0x70, 0x6F, 0x72, 0x74, 0x65, 0x64,  // Exported
            /* 0038 */  0x20, 0x44, 0x61, 0x74, 0x61, 0x56, 0x61, 0x75,  //  DataVau
            /* 0040 */  0x6C, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // lt......
            /* 0048 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0050 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0058 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0060 */  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // ........
            /* 0068 */  0x00, 0x00, 0x00, 0x00, 0xEE, 0x47, 0x19, 0x34,  // .....G.4
            /* 0070 */  0x71, 0xC8, 0xD5, 0x97, 0x45, 0xD2, 0xDE, 0x74,  // q...E..t
            /* 0078 */  0xFB, 0xC2, 0x12, 0x4B, 0x52, 0x28, 0x11, 0x16,  // ...KR(..
            /* 0080 */  0xFC, 0xF1, 0xBB, 0xD2, 0xFC, 0x25, 0xF0, 0xA0,  // .....%..
            /* 0088 */  0xC4, 0xF7, 0x12, 0x74, 0xB8, 0x06, 0x00, 0x00,  // ...t....
            /* 0090 */  0x52, 0x45, 0x50, 0x4F, 0x5D, 0x00, 0x00, 0x00,  // REPO]...
            /* 0098 */  0x01, 0x0B, 0x6B, 0x00, 0x00, 0x00, 0x00, 0x00,  // ..k.....
            /* 00A0 */  0x00, 0x00, 0x72, 0x87, 0xCD, 0xFF, 0x6D, 0x24,  // ..r...m$
            /* 00A8 */  0x47, 0xDB, 0x3D, 0x24, 0x92, 0xB4, 0x16, 0x6F,  // G.=$...o
            /* 00B0 */  0x45, 0xD8, 0xC3, 0xF5, 0x66, 0x14, 0x9F, 0x22,  // E...f.."
            /* 00B8 */  0xD7, 0xF7, 0xDE, 0x67, 0x90, 0x9A, 0xA2, 0x0D,  // ...g....
            /* 00C0 */  0x39, 0x25, 0xAD, 0xC3, 0x1A, 0xAD, 0x52, 0x0B,  // 9%....R.
            /* 00C8 */  0x75, 0x38, 0xE1, 0xA4, 0x14, 0x42, 0xB4, 0x53,  // u8...B.S
            /* 00D0 */  0xC1, 0x42, 0x2C, 0xC2, 0xF3, 0xF4, 0x77, 0x0E,  // .B,...w.
            /* 00D8 */  0x83, 0x65, 0x0B, 0x3A, 0x08, 0x29, 0x11, 0xBA,  // .e.:.)..
            /* 00E0 */  0xCC, 0x47, 0x7F, 0x5A, 0x38, 0x5F, 0x9F, 0x18,  // .G.Z8_..
            /* 00E8 */  0xA3, 0x3C, 0xE3, 0xF0, 0x10, 0xE0, 0x25, 0x88,  // .<....%.
            /* 00F0 */  0xA8, 0xB2, 0xF7, 0xC4, 0x46, 0x8D, 0x15, 0xFB,  // ....F...
            /* 00F8 */  0xF6, 0x6A, 0xAA, 0x0A, 0x74, 0xB4, 0x3B, 0x33,  // .j..t.;3
            /* 0100 */  0x1C, 0x38, 0x4E, 0xF6, 0xB0, 0x6C, 0x04, 0x4D,  // .8N..l.M
            /* 0108 */  0x82, 0x91, 0xC9, 0x65, 0x02, 0xF6, 0x01, 0xD5,  // ...e....
            /* 0110 */  0x9C, 0x9A, 0x4F, 0x10, 0xA2, 0xD4, 0x33, 0xA5,  // ..O...3.
            /* 0118 */  0x10, 0xE5, 0x1A, 0x86, 0xC3, 0xD4, 0xB4, 0x78,  // .......x
            /* 0120 */  0xC2, 0x89, 0x77, 0x37, 0xC5, 0x46, 0x6A, 0xD4,  // ..w7.Fj.
            /* 0128 */  0x16, 0x1F, 0x61, 0x90, 0x03, 0x29, 0xAF, 0xDC,  // ..a..)..
            /* 0130 */  0xDB, 0xCE, 0x64, 0xCF, 0x5F, 0x01, 0xB3, 0x1A,  // ..d._...
            /* 0138 */  0x50, 0x25, 0x2A, 0xBF, 0xD3, 0x66, 0x15, 0xCA,  // P%*..f..
            /* 0140 */  0xDF, 0x17, 0x7B, 0x48, 0x80, 0x4F, 0xAD, 0x58,  // ..{H.O.X
            /* 0148 */  0xA6, 0xB4, 0xB7, 0xDC, 0xD0, 0xD8, 0xF9, 0x14,  // ........
            /* 0150 */  0x2D, 0xA0, 0x0D, 0x79, 0xDB, 0x58, 0xD6, 0x92,  // -..y.X..
            /* 0158 */  0xAC, 0x39, 0x14, 0xFE, 0xDF, 0xE8, 0x9C, 0xB9,  // .9......
            /* 0160 */  0x0F, 0x4E, 0x38, 0x5A, 0x00, 0xB3, 0x62, 0xEB,  // .N8Z..b.
            /* 0168 */  0x9C, 0xF0, 0x7B, 0x48, 0x18, 0x7D, 0x60, 0x47,  // ..{H.}`G
            /* 0170 */  0xCC, 0x4E, 0x56, 0x66, 0x55, 0x9F, 0x81, 0x9A,  // .NVfU...
            /* 0178 */  0xCB, 0x8A, 0x7B, 0x2E, 0xB2, 0x16, 0x38, 0x90,  // ..{...8.
            /* 0180 */  0x69, 0xA4, 0x82, 0x71, 0xE1, 0xB5, 0xE0, 0x4D,  // i..q...M
            /* 0188 */  0xED, 0x82, 0x7F, 0x31, 0x98, 0xE0, 0x51, 0x9F,  // ...1..Q.
            /* 0190 */  0x9A, 0xB4, 0x74, 0x33, 0x9E, 0x9C, 0xC6, 0x16,  // ..t3....
            /* 0198 */  0xD6, 0x0A, 0x71, 0x7B, 0x57, 0x01, 0x22, 0x38,  // ..q{W."8
            /* 01A0 */  0xED, 0x7A, 0xC0, 0xFE, 0xC4, 0x75, 0x8C, 0x2C,  // .z...u.,
            /* 01A8 */  0xEA, 0x5C, 0x35, 0xAE, 0x84, 0xB6, 0x40, 0x6B,  // .\5...@k
            /* 01B0 */  0xB8, 0x7C, 0x64, 0xAC, 0xAE, 0xDF, 0xB8, 0x95,  // .|d.....
            /* 01B8 */  0xB8, 0x82, 0x10, 0xBA, 0x70, 0x5B, 0x62, 0x03,  // ....p[b.
            /* 01C0 */  0x29, 0x1D, 0x55, 0x78, 0x6B, 0xA4, 0x6B, 0x93,  // ).Uxk.k.
            /* 01C8 */  0x5A, 0xBC, 0x00, 0xF8, 0x65, 0x21, 0xA7, 0x38,  // Z...e!.8
            /* 01D0 */  0xAB, 0x59, 0x87, 0xD5, 0x4E, 0x2B, 0x38, 0x1C,  // .Y..N+8.
            /* 01D8 */  0xC4, 0xBC, 0x74, 0x0D, 0x50, 0x3C, 0xD4, 0x88,  // ..t.P<..
            /* 01E0 */  0xEF, 0x86, 0x2F, 0x1A, 0x05, 0x9E, 0x09, 0x2B,  // ../....+
            /* 01E8 */  0xC2, 0x6D, 0x7D, 0xD7, 0x8D, 0x37, 0x0A, 0x8B,  // .m}..7..
            /* 01F0 */  0x95, 0x7E, 0xA2, 0xF0, 0xB8, 0x38, 0xDC, 0x18,  // .~...8..
            /* 01F8 */  0x26, 0xA4, 0xC2, 0xDD, 0xCC, 0x8C, 0x68, 0xCB,  // &.....h.
            /* 0200 */  0xB2, 0xE4, 0x86, 0x6E, 0x74, 0xEF, 0x45, 0x06,  // ...nt.E.
            /* 0208 */  0xBC, 0xC4, 0xC4, 0xA0, 0x7D, 0xD5, 0xA5, 0xD4,  // ....}...
            /* 0210 */  0xA2, 0xFD, 0xEE, 0xA8, 0x55, 0x39, 0x24, 0x16,  // ....U9$.
            /* 0218 */  0x03, 0xBF, 0xB7, 0x6C, 0x46, 0xB3, 0xD3, 0x47,  // ...lF..G
            /* 0220 */  0x6A, 0xBE, 0xEF, 0x96, 0xF0, 0x53, 0x90, 0x81,  // j....S..
            /* 0228 */  0xC5, 0xEF, 0x47, 0x9B, 0x03, 0x0C, 0xC2, 0x43,  // ..G....C
            /* 0230 */  0x66, 0x8E, 0x7C, 0x16, 0x15, 0xE2, 0x68, 0x6B,  // f.|...hk
            /* 0238 */  0x0A, 0x31, 0x4D, 0x6A, 0x6B, 0xC2, 0x25, 0x84,  // .1Mjk.%.
            /* 0240 */  0x8E, 0x8A, 0xB5, 0xF8, 0xA5, 0xC7, 0x2B, 0xBE,  // ......+.
            /* 0248 */  0x58, 0x82, 0x6E, 0xED, 0xF8, 0x03, 0x6F, 0xC5,  // X.n...o.
            /* 0250 */  0x63, 0xC0, 0xD0, 0x42, 0x9A, 0x86, 0x22, 0xD3,  // c..B..".
            /* 0258 */  0xA4, 0xA8, 0x8C, 0x26, 0x5C, 0x80, 0xA7, 0x9C,  // ...&\...
            /* 0260 */  0x93, 0x5C, 0xAF, 0xE2, 0x0E, 0xCC, 0xCD, 0x8B,  // .\......
            /* 0268 */  0x19, 0xBB, 0xA4, 0x2F, 0xB4, 0xBA, 0x23, 0x71,  // .../..#q
            /* 0270 */  0xB0, 0x43, 0x80, 0x4A, 0xA9, 0x8A, 0x86, 0x28,  // .C.J...(
            /* 0278 */  0xB3, 0x95, 0x2E, 0xB1, 0xE1, 0x0C, 0x6C, 0x53,  // ......lS
            /* 0280 */  0x00, 0xE3, 0xDC, 0x65, 0x81, 0x72, 0xEE, 0x30,  // ...e.r.0
            /* 0288 */  0x09, 0xCD, 0x7A, 0x3A, 0x15, 0x7A, 0xB4, 0x8A,  // ..z:.z..
            /* 0290 */  0x19, 0x53, 0xB2, 0x22, 0x06, 0xF7, 0x51, 0x00,  // .S."..Q.
            /* 0298 */  0xFC, 0x61, 0x1B, 0xA2, 0xF3, 0x27, 0xE6, 0xCF,  // .a...'..
            /* 02A0 */  0x25, 0xA0, 0x8E, 0x0A, 0xBB, 0x48, 0x58, 0x5B,  // %....HX[
            /* 02A8 */  0x95, 0xAF, 0xF4, 0x44, 0x82, 0x3D, 0x2B, 0x0A,  // ...D.=+.
            /* 02B0 */  0x73, 0x4B, 0xB1, 0x11, 0x9A, 0x10, 0x07, 0x66,  // sK.....f
            /* 02B8 */  0x39, 0x12, 0xEE, 0xA2, 0x6C, 0x0D, 0x77, 0xB9,  // 9...l.w.
            /* 02C0 */  0xDA, 0x57, 0x0F, 0x3C, 0xB0, 0xC5, 0x68, 0x1F,  // .W.<..h.
            /* 02C8 */  0xE5, 0x3D, 0x9C, 0x59, 0xE7, 0x5D, 0x25, 0x7F,  // .=.Y.]%.
            /* 02D0 */  0x1B, 0x86, 0x9E, 0x48, 0x85, 0xB6, 0x7D, 0x1E,  // ...H..}.
            /* 02D8 */  0xEA, 0x0E, 0x35, 0x44, 0x82, 0x0A, 0xA6, 0x88,  // ..5D....
            /* 02E0 */  0x96, 0x44, 0xB8, 0xC5, 0xC8, 0x66, 0x7E, 0x08,  // .D...f~.
            /* 02E8 */  0xCE, 0xC2, 0xDD, 0x75, 0x1F, 0x3A, 0xA6, 0x0A,  // ...u.:..
            /* 02F0 */  0x74, 0xB2, 0x50, 0x59, 0xF9, 0x34, 0x51, 0x1B,  // t.PY.4Q.
            /* 02F8 */  0xAD, 0xA4, 0xB0, 0x3E, 0x94, 0x03, 0x14, 0x2E,  // ...>....
            /* 0300 */  0x9B, 0xFB, 0x33, 0x94, 0x97, 0x72, 0x92, 0x01,  // ..3..r..
            /* 0308 */  0x46, 0x2C, 0x21, 0x80, 0xBF, 0x3F, 0x6B, 0x66,  // F,!..?kf
            /* 0310 */  0xEE, 0xE0, 0x7A, 0xAF, 0x82, 0xA6, 0x95, 0x37,  // ..z....7
            /* 0318 */  0xFD, 0x83, 0x74, 0x14, 0x36, 0x14, 0xFB, 0xA0,  // ..t.6...
            /* 0320 */  0xEB, 0x08, 0xAC, 0xDB, 0x20, 0x6A, 0x17, 0x8A,  // .... j..
            /* 0328 */  0x36, 0x2B, 0xDF, 0x33, 0xD1, 0x8B, 0xA2, 0x95,  // 6+.3....
            /* 0330 */  0x66, 0xE8, 0x8C, 0x68, 0x4B, 0x4D, 0x6D, 0x13,  // f..hKMm.
            /* 0338 */  0x36, 0xBB, 0xEB, 0xE7, 0x94, 0x98, 0xF7, 0x7C,  // 6......|
            /* 0340 */  0x29, 0x24, 0x35, 0x12, 0x25, 0xD6, 0x3C, 0x41,  // )$5.%.<A
            /* 0348 */  0xED, 0x2C, 0x49, 0xB2, 0x93, 0x4C, 0xE6, 0xC2,  // .,I..L..
            /* 0350 */  0x63, 0x6A, 0x2C, 0x79, 0xE4, 0x78, 0xD1, 0xAF,  // cj,y.x..
            /* 0358 */  0x53, 0x0D, 0x8E, 0x72, 0x30, 0xB9, 0x0B, 0x28,  // S..r0..(
            /* 0360 */  0x30, 0x92, 0x2C, 0x3F, 0x42, 0x82, 0x5E, 0xE9,  // 0.,?B.^.
            /* 0368 */  0x1F, 0x7E, 0xE3, 0x5D, 0x21, 0xC4, 0x2C, 0x80,  // .~.]!.,.
            /* 0370 */  0x5A, 0xEF, 0x79, 0x69, 0x97, 0x9E, 0x14, 0x7D,  // Z.yi...}
            /* 0378 */  0x4E, 0x1D, 0x7B, 0xCA, 0x0A, 0x47, 0x5D, 0x65,  // N.{..G]e
            /* 0380 */  0xE2, 0x98, 0x36, 0x2B, 0x59, 0xDE, 0x91, 0x2E,  // ..6+Y...
            /* 0388 */  0x93, 0x6F, 0x5E, 0x3B, 0x38, 0xBA, 0xFB, 0x1F,  // .o^;8...
            /* 0390 */  0x33, 0x49, 0xA9, 0x2C, 0xD2, 0xD0, 0xC7, 0xF0,  // 3I.,....
            /* 0398 */  0x05, 0x7C, 0xF8, 0x77, 0x18, 0xAF, 0xF3, 0xD7,  // .|.w....
            /* 03A0 */  0x76, 0x60, 0x95, 0x05, 0xBA, 0x3C, 0xD3, 0xD8,  // v`...<..
            /* 03A8 */  0x43, 0x54, 0x11, 0xEB, 0xF4, 0x93, 0x0D, 0x58,  // CT.....X
            /* 03B0 */  0x29, 0xCF, 0x62, 0x51, 0x2A, 0xAC, 0x1B, 0x06,  // ).bQ*...
            /* 03B8 */  0xC1, 0x87, 0x33, 0xB7, 0x6B, 0xE2, 0xD0, 0x58,  // ..3.k..X
            /* 03C0 */  0x92, 0xC6, 0x32, 0x31, 0x26, 0xE6, 0xF9, 0x38,  // ..21&..8
            /* 03C8 */  0xE4, 0x59, 0x75, 0xC2, 0xB9, 0x70, 0xCB, 0xCD,  // .Yu..p..
            /* 03D0 */  0x8E, 0xEA, 0xE8, 0x31, 0xA3, 0x3C, 0xDB, 0xFD,  // ...1.<..
            /* 03D8 */  0x49, 0x68, 0xEB, 0x11, 0x33, 0x7C, 0x85, 0xDE,  // Ih..3|..
            /* 03E0 */  0x98, 0x61, 0x56, 0xDD, 0xBD, 0xFB, 0x9B, 0x66,  // .aV....f
            /* 03E8 */  0x42, 0x65, 0x20, 0xB1, 0x3B, 0x1F, 0x86, 0x7A,  // Be .;..z
            /* 03F0 */  0x6C, 0x7B, 0x20, 0x88, 0xBB, 0x02, 0x72, 0x26,  // l{ ...r&
            /* 03F8 */  0xF8, 0x49, 0x73, 0x1F, 0x11, 0xA6, 0xCE, 0xFD,  // .Is.....
            /* 0400 */  0x04, 0x9E, 0xA9, 0x01, 0xB3, 0x7F, 0xF9, 0x42,  // .......B
            /* 0408 */  0x1E, 0xC0, 0x3A, 0xDD, 0x8F, 0x00, 0x77, 0x55,  // ..:...wU
            /* 0410 */  0xD8, 0x33, 0xA1, 0x7D, 0x34, 0xCC, 0x40, 0x47,  // .3.}4.@G
            /* 0418 */  0x0D, 0xA1, 0x55, 0xB8, 0x5B, 0x97, 0x57, 0xF5,  // ..U.[.W.
            /* 0420 */  0xDB, 0x61, 0x44, 0xD9, 0x82, 0xEF, 0xDF, 0x9C,  // .aD.....
            /* 0428 */  0xE0, 0x9E, 0x5C, 0x1F, 0x5F, 0x94, 0x3D, 0x0D,  // ..\._.=.
            /* 0430 */  0x76, 0x67, 0x14, 0xC7, 0x48, 0x29, 0x6A, 0x5C,  // vg..H)j\
            /* 0438 */  0xBC, 0xF2, 0x0E, 0x05, 0x41, 0xBB, 0xE4, 0xC6,  // ....A...
            /* 0440 */  0x4B, 0x30, 0x59, 0x2A, 0x55, 0xA8, 0xF4, 0x2C,  // K0Y*U..,
            /* 0448 */  0xB7, 0x91, 0xC1, 0x72, 0x19, 0x76, 0x05, 0xEA,  // ...r.v..
            /* 0450 */  0x7D, 0xA4, 0x1B, 0x42, 0x89, 0x19, 0x55, 0x8A,  // }..B..U.
            /* 0458 */  0xCF, 0x70, 0x77, 0xEC, 0xF5, 0x97, 0x1D, 0x6C,  // .pw....l
            /* 0460 */  0xA0, 0x4F, 0x3A, 0x9A, 0x36, 0xCD, 0x16, 0x6C,  // .O:.6..l
            /* 0468 */  0xB2, 0xFE, 0x5F, 0x3F, 0xAD, 0xF8, 0xD0, 0x29,  // .._?...)
            /* 0470 */  0x20, 0x3F, 0x30, 0x79, 0x80, 0x6C, 0x10, 0x36,  //  ?0y.l.6
            /* 0478 */  0xBD, 0x4E, 0xF5, 0x20, 0xDA, 0x59, 0x27, 0x53,  // .N. .Y'S
            /* 0480 */  0xBA, 0x2E, 0x2C, 0x89, 0xFE, 0xE5, 0xB9, 0xD8,  // ..,.....
            /* 0488 */  0x15, 0x09, 0x98, 0xB9, 0x5D, 0x82, 0x48, 0xB3,  // ....].H.
            /* 0490 */  0xC0, 0xB3, 0x81, 0x21, 0xEC, 0x0E, 0xB3, 0xBE,  // ...!....
            /* 0498 */  0xFC, 0xA2, 0x1C, 0xC9, 0xF3, 0xE0, 0xE7, 0xF9,  // ........
            /* 04A0 */  0x10, 0x3E, 0xE6, 0x9D, 0xC8, 0xEB, 0xCE, 0xEE,  // .>......
            /* 04A8 */  0xA7, 0xCF, 0x8D, 0x54, 0x33, 0x8C, 0x6E, 0xAD,  // ...T3.n.
            /* 04B0 */  0x98, 0x5C, 0xB5, 0x33, 0xB2, 0x66, 0x4D, 0x48,  // .\.3.fMH
            /* 04B8 */  0xB0, 0x86, 0xF5, 0x9D, 0x5C, 0xA0, 0x84, 0xD7,  // ....\...
            /* 04C0 */  0x0E, 0xEE, 0x2A, 0x9D, 0x8E, 0xF8, 0x8C, 0x5A,  // ..*....Z
            /* 04C8 */  0x95, 0x42, 0x31, 0xE8, 0xCF, 0x71, 0x7A, 0xC5,  // .B1..qz.
            /* 04D0 */  0x95, 0x08, 0xE1, 0x78, 0x80, 0xA5, 0xFE, 0xA3,  // ...x....
            /* 04D8 */  0xE3, 0x20, 0x6C, 0x1D, 0x37, 0xF1, 0xAC, 0xCD,  // . l.7...
            /* 04E0 */  0xE5, 0xEA, 0xD9, 0x00, 0x3C, 0x97, 0xE6, 0xEA,  // ....<...
            /* 04E8 */  0x4B, 0x85, 0x40, 0x2D, 0xE8, 0xC6, 0x1D, 0x51,  // K.@-...Q
            /* 04F0 */  0xED, 0xCE, 0x16, 0xFD, 0x12, 0xD7, 0x9F, 0xD5,  // ........
            /* 04F8 */  0x09, 0xA8, 0x48, 0xB9, 0xCC, 0x9E, 0xDA, 0xDD,  // ..H.....
            /* 0500 */  0x69, 0x40, 0xF7, 0x7A, 0x29, 0x0B, 0x25, 0x33,  // i@.z).%3
            /* 0508 */  0x6B, 0x37, 0xA5, 0xA7, 0xA4, 0x70, 0xCE, 0x25,  // k7...p.%
            /* 0510 */  0x4C, 0xF6, 0x73, 0x91, 0x29, 0x22, 0x28, 0x25,  // L.s.)"(%
            /* 0518 */  0x1F, 0x35, 0x8D, 0x1B, 0xBD, 0x0E, 0x10, 0xDB,  // .5......
            /* 0520 */  0xFF, 0xAC, 0x58, 0xAB, 0x7E, 0xAA, 0xB6, 0x90,  // ..X.~...
            /* 0528 */  0xB8, 0xF1, 0x21, 0x80, 0x20, 0xC8, 0x3A, 0x62,  // ..!. .:b
            /* 0530 */  0xF1, 0xC2, 0xC7, 0x96, 0xB2, 0xFE, 0x26, 0x59,  // ......&Y
            /* 0538 */  0xFC, 0x69, 0x30, 0x44, 0x95, 0x1D, 0x98, 0xDC,  // .i0D....
            /* 0540 */  0x01, 0x3A, 0xC3, 0x33, 0xAE, 0x41, 0x4B, 0x05,  // .:.3.AK.
            /* 0548 */  0x8C, 0x8A, 0xC8, 0xF6, 0xB8, 0xBF, 0x23, 0xAB,  // ......#.
            /* 0550 */  0x64, 0x70, 0x29, 0xA1, 0x9C, 0x7A, 0xB9, 0x54,  // dp)..z.T
            /* 0558 */  0xFE, 0x3A, 0x96, 0xB5, 0x8B, 0xFA, 0xA3, 0x9B,  // .:......
            /* 0560 */  0x38, 0xB9, 0x63, 0x16, 0xBF, 0xA8, 0x2E, 0x12,  // 8.c.....
            /* 0568 */  0x20, 0x43, 0x52, 0xBD, 0x05, 0xA9, 0xD2, 0xEA,  //  CR.....
            /* 0570 */  0xE6, 0x76, 0xC6, 0x1E, 0x4D, 0x3B, 0x2A, 0xBB,  // .v..M;*.
            /* 0578 */  0x5C, 0xF6, 0xAA, 0xDA, 0xF5, 0x91, 0x0A, 0xDA,  // \.......
            /* 0580 */  0x93, 0x18, 0x43, 0x59, 0x2E, 0x21, 0x81, 0x63,  // ..CY.!.c
            /* 0588 */  0x7B, 0x62, 0xA3, 0xAC, 0x4B, 0xFF, 0x25, 0x02,  // {b..K.%.
            /* 0590 */  0x81, 0xA4, 0xE0, 0xFE, 0xB8, 0x1C, 0x08, 0x8D,  // ........
            /* 0598 */  0x80, 0x09, 0x88, 0x36, 0xC4, 0x5D, 0x62, 0x96,  // ...6.]b.
            /* 05A0 */  0x52, 0xAE, 0x8B, 0x5C, 0x6E, 0xA9, 0x62, 0xD8,  // R..\n.b.
            /* 05A8 */  0x62, 0x14, 0xC0, 0xDD, 0xD0, 0x1A, 0x06, 0xD5,  // b.......
            /* 05B0 */  0xF1, 0x43, 0xDA, 0x91, 0xF1, 0x37, 0xEB, 0x0B,  // .C...7..
            /* 05B8 */  0xC3, 0x9B, 0xB3, 0x78, 0xF8, 0x12, 0x27, 0xC6,  // ...x..'.
            /* 05C0 */  0x70, 0xFA, 0x22, 0x78, 0xB5, 0x3A, 0x46, 0xCE,  // p."x.:F.
            /* 05C8 */  0x53, 0xD7, 0xF3, 0x55, 0xFF, 0x3D, 0xF4, 0xF8,  // S..U.=..
            /* 05D0 */  0xAD, 0x00, 0xEA, 0xAF, 0xCA, 0x73, 0x03, 0x91,  // .....s..
            /* 05D8 */  0x19, 0x93, 0x7D, 0x34, 0x64, 0xC1, 0xDF, 0xA4,  // ..}4d...
            /* 05E0 */  0x67, 0xC8, 0x43, 0x10, 0xF3, 0xC2, 0xB9, 0x13,  // g.C.....
            /* 05E8 */  0x14, 0xAD, 0xBE, 0x4D, 0x4B, 0x88, 0xB8, 0xB7,  // ...MK...
            /* 05F0 */  0x2E, 0x10, 0xFF, 0x47, 0x94, 0xCC, 0x19, 0xB0,  // ...G....
            /* 05F8 */  0x16, 0xFF, 0xF9, 0xC6, 0x0F, 0x7D, 0x12, 0x4E,  // .....}.N
            /* 0600 */  0xD3, 0x16, 0x6E, 0x4E, 0x00, 0x07, 0xE1, 0xBF,  // ..nN....
            /* 0608 */  0x3B, 0x89, 0xBE, 0x4E, 0xCA, 0xAA, 0xFF, 0xC9,  // ;..N....
            /* 0610 */  0xAA, 0x55, 0x79, 0x43, 0x5D, 0x66, 0xF4, 0x32,  // .UyC]f.2
            /* 0618 */  0xBF, 0x40, 0x32, 0x4C, 0xA7, 0xF0, 0x60, 0xB6,  // .@2L..`.
            /* 0620 */  0x89, 0xF1, 0xC7, 0xEB, 0x00, 0x60, 0xAB, 0xD6,  // .....`..
            /* 0628 */  0xDE, 0xBE, 0x2E, 0x7E, 0x58, 0xF9, 0x31, 0xE8,  // ...~X.1.
            /* 0630 */  0x88, 0x55, 0xBE, 0xD8, 0xEF, 0x89, 0x40, 0xC0,  // .U....@.
            /* 0638 */  0x91, 0xAB, 0x17, 0xCD, 0x0F, 0x58, 0x82, 0xCC,  // .....X..
            /* 0640 */  0xBC, 0xC9, 0x57, 0x38, 0xC1, 0x2D, 0xA9, 0x1E,  // ..W8.-..
            /* 0648 */  0x1E, 0x8C, 0xE8, 0xC1, 0x2C, 0x30, 0x26, 0xE3,  // ....,0&.
            /* 0650 */  0x4A, 0x5B, 0x94, 0xC4, 0x5E, 0xAD, 0x34, 0x2B,  // J[..^.4+
            /* 0658 */  0xCE, 0x50, 0x94, 0x14, 0x30, 0x3D, 0x47, 0x91,  // .P..0=G.
            /* 0660 */  0xE9, 0x71, 0x9F, 0x43, 0x33, 0x0E, 0x51, 0xCB,  // .q.C3.Q.
            /* 0668 */  0xC5, 0x67, 0x0A, 0x58, 0x98, 0x40, 0x1A, 0xCA,  // .g.X.@..
            /* 0670 */  0x00, 0x12, 0xBA, 0xA2, 0xF8, 0xF2, 0xEE, 0xA6,  // ........
            /* 0678 */  0xE5, 0x58, 0x61, 0xB3, 0xE7, 0x54, 0xAC, 0x96,  // .Xa..T..
            /* 0680 */  0xAF, 0x43, 0x08, 0xB1, 0x75, 0x79, 0xBE, 0x20,  // .C..uy. 
            /* 0688 */  0xE8, 0xB9, 0x8F, 0xFF, 0x90, 0x94, 0x2A, 0xA0,  // ......*.
            /* 0690 */  0xFA, 0x60, 0x0A, 0x49, 0x48, 0x78, 0x0A, 0xC4,  // .`.IHx..
            /* 0698 */  0xE1, 0xC0, 0x26, 0x86, 0xEA, 0x25, 0xC5, 0x97,  // ..&..%..
            /* 06A0 */  0x0A, 0x6C, 0x56, 0x1A, 0x2D, 0x0F, 0xB9, 0x47,  // .lV.-..G
            /* 06A8 */  0x71, 0x4A, 0xF5, 0xFF, 0x75, 0xFD, 0x3C, 0x24,  // qJ..u.<$
            /* 06B0 */  0xCD, 0x7C, 0xE9, 0xFE, 0x7E, 0xC0, 0x18, 0x83,  // .|..~...
            /* 06B8 */  0xD8, 0x1F, 0xB8, 0x3B, 0xE3, 0x97, 0xEA, 0x28,  // ...;...(
            /* 06C0 */  0x4A, 0x97, 0x13, 0x24, 0x1A, 0x93, 0x21, 0x02,  // J..$..!.
            /* 06C8 */  0x4D, 0xE1, 0x11, 0x14, 0x3F, 0x5F, 0xD3, 0xC4,  // M...?_..
            /* 06D0 */  0x02, 0x5E, 0x60, 0x11, 0x53, 0xF9, 0xB3, 0x6C,  // .^`.S..l
            /* 06D8 */  0x6F, 0x73, 0xC5, 0xF7, 0xD4, 0xE1, 0x4E, 0x77,  // os....Nw
            /* 06E0 */  0xAE, 0x00, 0x62, 0xBF, 0x62, 0xF1, 0x38, 0x1D,  // ..b.b.8.
            /* 06E8 */  0x7D, 0xC3, 0xCE, 0x16, 0x65, 0xE8, 0xA0, 0xA7,  // }...e...
            /* 06F0 */  0x7D, 0x12, 0xB2, 0x13, 0xC4, 0xD4, 0x25, 0xE0,  // }.....%.
            /* 06F8 */  0x68, 0xD3, 0xB7, 0x4A, 0xD5, 0x4F, 0x5B, 0x7C,  // h..J.O[|
            /* 0700 */  0xF1, 0xB1, 0x25, 0xAE, 0xA5, 0x8F, 0x92, 0xEE,  // ..%.....
            /* 0708 */  0x70, 0x87, 0x25, 0x8A, 0xB7, 0x04, 0xA3, 0xA6,  // p.%.....
            /* 0710 */  0x89, 0x1F, 0x7E, 0xBB, 0x9E, 0x54, 0x05, 0x37,  // ..~..T.7
            /* 0718 */  0x18, 0x9A, 0x77, 0x16, 0x50, 0x16, 0x73, 0x0E,  // ..w.P.s.
            /* 0720 */  0x5C, 0x76, 0xF2, 0xD1, 0x3D, 0x57, 0x98, 0x08,  // \v..=W..
            /* 0728 */  0xA6, 0xC6, 0x74, 0xD9, 0x58, 0xCA, 0x33, 0x7E,  // ..t.X.3~
            /* 0730 */  0x30, 0xE4, 0x0E, 0x0D, 0xB3, 0xBD, 0xD4, 0x25,  // 0......%
            /* 0738 */  0xC6, 0xC9, 0x64, 0x23, 0x0F, 0x19, 0x22, 0xDF,  // ..d#..".
            /* 0740 */  0xD7, 0xBE, 0xCA, 0x83, 0xCC, 0x3F, 0x78, 0xDA,  // .....?x.
            /* 0748 */  0x18, 0x76, 0xE6, 0x4A                           // .v.J
        }
    })
    Method (DBDV, 0, NotSerialized)
    {
        If ((((BMID == 0x03) || (BMID == 0x04)) || (BMID == 0x05)))
        {
            Return (DBD1) /* \DBD1 */
        }
        Else
        {
            Return (DBD0) /* \DBD0 */
        }
    }

    Scope (\_SB.IETM)
    {
        Method (TEVT, 2, Serialized)
        {
            Switch (Arg0)
            {
                Case ("IETM")
                {
                    Notify (\_SB.IETM, Arg1)
                }
                Case ("B0D4")
                {
                    Notify (\_SB.PCI0.B0D4, Arg1)
                }

            }
        }
    }

    Scope (\_SB.IETM)
    {
        Method (GDDV, 0, Serialized)
        {
            Return (DBDV ())
        }

        Method (IMOK, 1, NotSerialized)
        {
            ADBG ("IMOK")
            ADBG (Arg0)
            Return (Arg0)
        }
    }
}

