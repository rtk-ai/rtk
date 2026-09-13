encia %TYPE,
                                pNtrc_Code         IN PTOVENTA.VTA_OPERACION_FUNO.NTRC_CODE%TYPE,
                                pNumPedidoVta      IN PTOVENTA.VTA_OPERACION_FUNO.NUM_PED_VTA%TYPE,
                                pEstComisionCash   IN PTOVENTA.VTA_OPERACION_FUNO.EST_COMISION_CASH%TYPE);
--FIN JPARCO 06052026
end PTOVENTA_FUNO;
/
create or replace package body PTOVENTA.PTOVENTA_FUNO is

  --INI CANTONIO 03/10/2019
  function EST_FUNO_RECAUDACION RETURN VARCHAR2 IS
    vRpta VARCHAR2(20);
  BEGIN
    SELECT LLAVE_TAB_GRAL
      INTO vRpta
      FROM PTOVENTA.PBL_TAB_GRAL PTO
     WHERE PTO.COD_TAB_GRAL = 'EST_FUNO'
       AND EST_TAB_GRAL = 'A';

    RETURN vRpta;

  END;
  -- FIN CANTONIO
  --INI SLEYVA 05/11/2019
  function COD_COMERCIO_FARMA RETURN VARCHAR2 IS
    vRpta VARCHAR2(20);
  BEGIN
    SELECT LLAVE_TAB_GRAL
      INTO vRpta
      FROM PTOVENTA.PBL_TAB_GRAL PTO
     WHERE PTO.COD_TAB_GRAL = 'COD_COMERCIO'
       AND EST_TAB_GRAL = 'A';

    RETURN vRpta;

  END;
  --INI SLEYVA 05/11/2019

  -- INI SLEYVA 11/10/2019
  function imprimirVoucherComprobacion(codGrupoCia IN VARCHAR2,
                                       codLocal    IN VARCHAR2,
                                       numero      IN VARCHAR2,
                                       monto       IN VARCHAR2)
    RETURN FarmaCursor IS
    curDataCupon FarmaCursor;
    vIdDoc       IMPRESION_TERMICA.ID_DOC%type;
    vIpPc        IMPRESION_TERMICA.IP_PC%type;
    vNumero      VARCHAR2(20);
    vMonto       VARCHAR(20);
  BEGIN

    vIdDoc  := FARMA_PRINTER.F_GENERA_ID_DOC;
    vIpPc   := FARMA_PRINTER.F_GET_IP_SESS;
    vNumero := 'Numero: ' || numero;
    vMonto  := 'Monto: S/.' || monto;

    FARMA_PRINTER.P_AGREGA_LOGO_MARCA(vIdDoc_in    => vIdDoc,
                                      vIpPc_in     => vIpPc,
                                      vCodGrupoCia => codGrupoCia,
                                      vCodLocal_in => codLocal);

    FARMA_PRINTER.P_AGREGA_LINEA_BLANCO(vIdDoc, vIpPc);

    FARMA_PRINTER.P_AGREGA_TEXTO(vIdDoc_in    => vIdDoc,
                                 vIpPc_in     => vIpPc,
                                 vValor_in    => vNumero,
                                 vTamanio_in  => FARMA_PRINTER.TAMANIO_3,
                                 vAlineado_in => FARMA_PRINTER.ALING_CEN,
                                 vNegrita_in  => FARMA_PRINTER.BOLD_ACT);
    FARMA_PRINTER.P_AGREGA_LINEA_BLANCO(vIdDoc, vIpPc);

    FARMA_PRINTER.P_AGREGA_TEXTO(vIdDoc_in    => vIdDoc,
                                 vIpPc_in     => vIpPc,
                                 vValor_in    => vMonto,
                                 vTamanio_in  => FARMA_PRINTER.TAMANIO_3,
                                 vAlineado_in => FARMA_PRINTER.ALING_CEN,
                                 vNegrita_in  => FARMA_PRINTER.BOLD_ACT);
    FARMA_PRINTER.P_AGREGA_LINEA_BLANCO(vIdDoc, vIpPc);

    curDataCupon := FARMA_PRINTER.F_CUR_OBTIENE_DOC_IMPRIMIR(vIdDoc, vIpPc);

    RETURN curDataCupon;
  END;
  -- FIN  SLEYVA 11/10/2019
  -- INI SLEYVA 09/10/2019
  PROCEDURE REGISTRA_OPERACION_FUNO(pCoCompania          IN PTOVENTA.VTA_OPERACION_FUNO.COD_GRUPO_CIA%TYPE,
                                    pCoLocal             IN PTOVENTA.VTA_OPERACION_FUNO.COD_LOCAL%TYPE,
                                    pDeRespondeCode      IN PTOVENTA.VTA_OPERACION_FUNO.DESC_RESPONDE_COD%TYPE,
                                    pVaMonto             IN VARCHAR2,
                                    pCoAprobacion        IN PTOVENTA.VTA_OPERACION_FUNO.COD_APROBACION%TYPE,
                                    pDeMensaje           IN PTOVENTA.VTA_OPERACION_FUNO.DES_MENSAJE%TYPE,
                                    pDeTarjeta           IN PTOVENTA.VTA_OPERACION_FUNO.DES_TARJETA%TYPE,
                                    pDeIdTarjeta         IN PTOVENTA.VTA_OPERACION_FUNO.DES_ID_TARJETA%TYPE,
                                    pNuCuotas            IN PTOVENTA.VTA_OPERACION_FUNO.NUM_CUOTAS%TYPE,
                                    pVaMontoCuota        IN VARCHAR2,
                                    pTiCredito           IN PTOVENTA.VTA_OPERACION_FUNO.TI_CREDITO%TYPE,
                                    pDeNombreCliente     IN PTOVENTA.VTA_OPERACION_FUNO.DES_NOMBRE_CLIENTE%TYPE,
                                    pDeCodigoMoneda      IN PTOVENTA.VTA_OPERACION_FUNO.DES_COD_MONEDA%TYPE,
                                    pDeAplicacion        IN PTOVENTA.VTA_OPERACION_FUNO.DES_APLICACION%TYPE,
                                    pDeTipoTransaccion   IN PTOVENTA.VTA_OPERACION_FUNO.DES_TIPO_TRANSACCION%TYPE,
                                    pCoTipoServicioFuno  IN PTOVENTA.VTA_OPERACION_FUNO.COD_TIPO_SERVICIO_FUNO%TYPE,
                                    pCoComercio          IN PTOVENTA.VTA_OPERACION_FUNO.COD_COMERCIO%TYPE,
                                    pCoAdquiriente       IN PTOVENTA.VTA_OPERACION_FUNO.COD_ADQUIRIENTE%TYPE,
                                    pIdAutorizacion      IN PTOVENTA.VTA_OPERACION_FUNO.ID_AUTORIZACION%TYPE,
                                    pDeDatetime          IN PTOVENTA.VTA_OPERACION_FUNO.DES_DATETIME%TYPE,
                                    pDeTipo              IN PTOVENTA.VTA_OPERACION_FUNO.DES_TIPO%TYPE,
                                    pTiLectura           IN PTOVENTA.VTA_OPERACION_FUNO.TI_LECTURA%TYPE,
                                    pDeTerminal          IN PTOVENTA.VTA_OPERACION_FUNO.DE_TERMINAL%TYPE,
                                    pNuLote              IN PTOVENTA.VTA_OPERACION_FUNO.NUM_LOTE%TYPE,
                                    pNuReferencia        IN PTOVENTA.VTA_OPERACION_FUNO.NUM_REFERENCIA%TYPE,
                                    pIdCreaOperacionFuno IN PTOVENTA.VTA_OPERACION_FUNO.COD_CREA_OPERACION_FUNO%TYPE,
                                    pCoCajero            IN PTOVENTA.VTA_OPERACION_FUNO.COD_CAJERO%TYPE,
                                    pNuCaja              IN PTOVENTA.VTA_OPERACION_FUNO.NUM_CAJA%TYPE,
                                    pNuTurno             IN PTOVENTA.VTA_OPERACION_FUNO.NUM_TURNO%TYPE,
                                    pDeOperador          IN PTOVENTA.VTA_OPERACION_FUNO.DES_OPERADOR%TYPE) AS
    V_CoTipoServicioFuno PTOVENTA.VTA_OPERACION_FUNO.COD_TIPO_SERVICIO_FUNO%TYPE;
  BEGIN
    --SE HA DEJADO EL MÉTODO IGUAL A INKAVENTA
    IF (LENGTH(TRIM(pCoTipoServicioFuno)) = 0) THEN
        V_CoTipoServicioFuno := NULL;
    ELSE
        V_CoTipoServicioFuno := pCoTipoServicioFuno;
    END IF;

    INSERT INTO PTOVENTA.VTA_OPERACION_FUNO
      (COD_GRUPO_CIA,
       COD_LOCAL,
       COD_TRANSACCION_FUNO,
       DESC_RESPONDE_COD,
       VAL_MONTO,
       COD_APROBACION,
       DES_MENSAJE,
       DES_TARJETA,
       DES_ID_TARJETA,
       NUM_CUOTAS,
       VA_MONTO_CUOTA,
       TI_CREDITO,
       DES_NOMBRE_CLIENTE,
       DES_COD_MONEDA,
       DES_APLICACION,
       DES_TIPO_TRANSACCION,
       COD_TIPO_SERVICIO_FUNO,
       COD_COMERCIO,
       COD_ADQUIRIENTE,
       ID_AUTORIZACION,
       DES_DATETIME,
       DES_TIPO,
       TI_LECTURA,
       DE_TERMINAL,
       NUM_LOTE,
       NUM_REFERENCIA,
       COD_CREA_OPERACION_FUNO,
       FEC_CREA_OPERACION_FUNO,
       COD_CAJERO,
       NUM_CAJA,
       NUM_TURNO,
       DES_OPERADOR)
    Values
      (pCoCompania,
       pCoLocal,
       SEQ_OPERACION_FUNO.NEXTVAL,
       pDeRespondeCode,
       TO_NUMBER(SUBSTR(pVaMonto, 0, LENGTH(pVaMonto) - 2) || '.' || SUBSTR(pVaMonto, LENGTH(pVaMonto) - 1), '9999999999999.99'),     --RARGUMEDO 21-05-2020
       pCoAprobacion,
       pDeTarjeta,        -- RARGUMEDO 26-05-2020
       pDeTarjeta,
       pDeIdTarjeta,
       pNuCuotas,
       TO_NUMBER(SUBSTR(pVaMontoCuota, 0, LENGTH(pVaMontoCuota) - 2) || '.' || SUBSTR(pVaMontoCuota, LENGTH(pVaMontoCuota) - 1), '9999999999999.99'),    -- RARGUMEDO 21-05-2020
       pTiCredito,
       pDeNombreCliente,
       pDeCodigoMoneda,
       pDeAplicacion,
       pDeTipoTransaccion,
       V_CoTipoServicioFuno,
       pCoComercio,
       pCoAdquiriente,
       pIdAutorizacion,
       pDeDateTime,
       pDeTipo,
       pTiLectura,
       pDeTerminal,
       pNuLote,
       pNuReferencia,
       pIdCreaOperacionFuno,
       sysdate,
       pCoCajero,
       pNuCaja,
       pNuTurno,
       pDeOperador);

  END;
  -- FIN SLEYVA 09/10/2019

  -- INI SLEYVA 09/10/2019
  FUNCTION GET_TRANSACCIONES_FUNO(codigocompania_in IN VTA_COMP_PAGO.COD_GRUPO_CIA%TYPE,
                                  codigolocal_in    IN VTA_COMP_PAGO.COD_LOCAL%TYPE,
                                  feTx_in           IN CHAR,
                                  feTx_fin          IN CHAR)
    RETURN FarmaCursor IS
    farmacur FarmaCursor;
  BEGIN
    OPEN farmacur FOR
      SELECT CASE
               WHEN A.DES_TIPO_TRANSACCION = '03' THEN
                'DISPOSICIÓN EFECTIVO'
               WHEN A.DES_TIPO_TRANSACCION = '06' THEN
                'ANULACIÓN DISPOSICIÓN EFECTIVO'
               WHEN A.DES_TIPO_TRANSACCION = '13' THEN
                'PAGO SERVICIOS'
               WHEN A.DES_TIPO_TRANSACCION = '07' THEN
                'ANULACIÓN PAGO SERVICIOS'
               ELSE
                ' '
             END || 'Ã' || NVL(A.COD_TIPO_SERVICIO_FUNO, ' ') || 'Ã' ||
             NVL(B.DES_TIPO_SERVICIO_FUNO, ' ') || 'Ã' ||
             TO_CHAR(A.VAL_MONTO, '999,999,999.99') || 'Ã' ||
             NVL(A.COD_APROBACION, ' ') || 'Ã' || NVL(A.DES_TARJETA, ' ') || 'Ã' ||
             NVL(A.DES_ID_TARJETA, ' ') || 'Ã' ||
             NVL(A.DES_NOMBRE_CLIENTE, ' ') || 'Ã' || NVL(A.NUM_LOTE, ' ') || 'Ã' ||
             NVL(A.NUM_REFERENCIA, ' ') || 'Ã' || NVL(A.COD_CAJERO, ' ') || 'Ã' ||
             TRIM(C.APE_PAT) || ' ' || TRIM(C.APE_MAT) || ' ' ||
             TRIM(C.NOM_USU) || 'Ã' || A.NUM_CAJA || 'Ã' || A.NUM_TURNO || 'Ã' ||
             A.DES_OPERADOR  || 'Ã' ||
			 NVL(A.IP_PC, ' ')
        FROM PTOVENTA.VTA_OPERACION_FUNO A,
             VTA_TIPO_SERVICIO_FUNO      B,
             PBL_USU_LOCAL               C
       WHERE A.COD_GRUPO_CIA = codigocompania_in
         AND A.COD_LOCAL = codigolocal_in
         AND A.FEC_CREA_OPERACION_FUNO BETWEEN
             TO_DATE(feTx_in || ' 00:00:00', 'DD/MM/YYYY HH24:MI:SS') AND
             TO_DATE(feTx_fin || ' 23:59:59', 'DD/MM/YYYY HH24:MI:SS')
         AND A.COD_TIPO_SERVICIO_FUNO = B.COD_TIPO_SERVICIO_FUNO(+)
         AND A.COD_GRUPO_CIA = C.COD_GRUPO_CIA
         AND A.COD_LOCAL = C.COD_LOCAL
         AND A.COD_CAJERO = C.SEC_USU_LOCAL;

    RETURN farmacur;
  END;
  -- FIN SLEYVA 09/10/2019

  -- INI SLEYVA 10/10/2019
  PROCEDURE GRABA_OPERACON_INVALIDA(codigogrupocia_in  IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.COD_GRUPO_CIA%TYPE,
                                    codigolocal_in     IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.COD_LOCAL%TYPE,
                                    cocajero_in        IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.COD_CAJERO%TYPE,
                                    coopesolicitada_in IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.COD_OPERACION_SOLICITADA%TYPE,
                                    cooperealizada_in  IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.COD_OPERACION_EFECTUADA%TYPE,
                                    vamonto_in         IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.VAL_MONTO%TYPE,
                                    idtransaccion_in   IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.COD_TRANSACCION%TYPE,
                                    cocomercio_in      IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.COD_COMERCIO%TYPE,
                                    determinal_in      IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.DES_TERMINAL%TYPE,
                                    nulote_in          IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.NUM_LOTE%TYPE,
                                    nureferencia_in    IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.NUM_REFERENCIA%TYPE,
                                    nuap_in            IN PTOVENTA.VTA_OPERACIONES_INVALIDAS.NUM_AP%TYPE) IS
    V_CO_SECUENCIAL NUMBER;
  BEGIN
    SELECT NVL(MAX(COD_SECUENCIAL), 0) + 1
      INTO V_CO_SECUENCIAL
      FROM PTOVENTA.VTA_OPERACIONES_INVALIDAS
     WHERE COD_GRUPO_CIA = codigogrupocia_in
       AND COD_LOCAL = CODIGOLOCAL_IN;

    INSERT INTO PTOVENTA.VTA_OPERACIONES_INVALIDAS
      (COD_GRUPO_CIA,
       COD_LOCAL,
       COD_SECUENCIAL,
       FEC_OPERACION,
       COD_CAJERO,
       COD_OPERACION_SOLICITADA,
       COD_OPERACION_EFECTUADA,
       VAL_MONTO,
       COD_TRANSACCION,
       COD_COMERCIO,
       DES_TERMINAL,
       NUM_LOTE,
       NUM_REFERENCIA,
       NUM_AP)
    VALUES
      (codigogrupocia_in,
       CODIGOLOCAL_IN,
       V_CO_SECUENCIAL,
       SYSDATE,
       COCAJERO_IN,
       COOPESOLICITADA_IN,
       COOPEREALIZADA_IN,
       VAMONTO_IN,
       IDTRANSACCION_IN,
       COCOMERCIO_IN,
       DETERMINAL_IN,
       NULOTE_IN,
       NUREFERENCIA_IN,
       NUAP_IN);

    COMMIT;
  END;
  -- FIN SLEYVA 10/10/2019
  --INI RARGUMEDO 10-02-2020 -- OPERACIONES AGORA
  PROCEDURE UPDATE_TIPO_RCD(pCoGrupoCia        IN PTOVENTA.VTA_OPERACION_FUNO.cod_grupo_cia%TYPE,
                            pCoCia             IN PTOVENTA.VTA_OPERACION_FUNO.COD_CIA %TYPE,
                            pCoLocal           IN PTOVENTA.VTA_OPERACION_FUNO.cod_local %TYPE,
                            pCoAprobacion      IN PTOVENTA.VTA_OPERACION_FUNO.cod_aprobacion %TYPE,
                            pDeTarjeta         IN PTOVENTA.VTA_OPERACION_FUNO.des_tarjeta %TYPE,
                            pDeTipoTransaccion IN PTOVENTA.VTA_OPERACION_FUNO.des_tipo_transaccion %TYPE,
                            pIdAutorizacion    IN PTOVENTA.VTA_OPERACION_FUNO.id_autorizacion %TYPE,
                            pNuLote            IN PTOVENTA.VTA_OPERACION_FUNO.num_lote %TYPE,
                            pNuReferencia      IN PTOVENTA.VTA_OPERACION_FUNO.num_referencia %TYPE,
                            pNtrc_Code         IN PTOVENTA.VTA_OPERACION_FUNO.NTRC_CODE%TYPE,
                            pTipR